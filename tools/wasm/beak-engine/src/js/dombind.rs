//! The DOM for JavaScript.
//!
//! `beak_engine::dom` is an owning tree (`Element` owns its children), in
//! which JavaScript cannot hold references. So the tree is flattened into an
//! arena: a `Vec` of nodes, and a handle is an index. Layout still reads
//! `dom::Dom`; `Doc::to_dom` writes the arena back so changes become
//! visible.
//!
//! Node identity is observable: `document.body === document.body` must be
//! true, so each node's JS wrapper is built once and kept.

use alloc::rc::Rc;
use hashbrown::HashMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::interp::*;
use super::value::*;

/// Void elements (no end tag).
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
pub const COMMENT_NODE: f64 = 8.0;
pub const DOCUMENT_NODE: f64 = 9.0;

pub struct DomNode {
    pub kind: f64,
    pub tag: Rc<str>,
    pub attrs: Vec<(Rc<str>, Rc<str>)>,
    pub text: Rc<str>,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Built once and kept, otherwise `el === el` would be false.
    pub js: Option<Gc>,
    /// Registered listeners, per event type.
    pub listeners: Vec<(Rc<str>, Value)>,
    /// Handlers set as a property (`el.onclick = f`).
    ///
    /// Separate from `listeners` because a second assignment replaces the
    /// first, while `addEventListener` appends. The property and the
    /// same-named attribute share one slot, as in browsers.
    pub handlers: Vec<(Rc<str>, Value)>,
    /// The dirty value of a control (HTML §4.10.5.5): what the user typed or a
    /// script set.
    ///
    /// Kept separate from the attribute: `el.value = x` does not change the
    /// `value` attribute, which stays the default value (`defaultValue`) that
    /// `form.reset()` restores.
    pub value: Option<Rc<str>>,
    /// The same for `checked`; `defaultChecked` is the attribute.
    pub checked: Option<bool>,
    /// `<template>` only: the fragment node holding its content. Created on
    /// the first `.content` access.
    pub content: Option<u32>,
    /// The `seq` of the same element in beak's tree: the bridge from a hit
    /// point to a node. `to_dom` assigns it and writes it back here; layout
    /// reports it on a hit.
    pub seq: u32,
    /// The `seq` of the element in the tree this document was built from.
    ///
    /// The bridge for `getComputedStyle`: the cascade runs on beak's tree
    /// (`crate::dom`), script works on the arena.
    ///
    /// `0` means no source node: an element created by script. For those
    /// `getComputedStyle` can only answer from the inline style.
    pub src_seq: u32,
    /// Custom element state (HTML §4.13.4), one of the `CE_*` constants.
    /// `CE_NONE` means "undefined" for a valid custom element name and
    /// "uncustomized" for any other.
    pub ce: u8,
    /// An element: its shadow root. A shadow root: its host. The root is not
    /// among the host's children and has no parent.
    pub shadow: Option<u32>,
    /// Shown states without an attribute: `UI_POPOVER_OPEN`, `UI_MODAL`.
    pub ui: u8,
    /// Custom states (`ElementInternals.states`, `:state()`).
    pub states: Vec<Rc<str>>,
}

pub const UI_POPOVER_OPEN: u8 = 1;
pub const UI_MODAL: u8 = 2;

impl DomNode {
    fn new(kind: f64, tag: &str) -> DomNode {
        DomNode { kind, tag: Rc::from(tag), attrs: Vec::new(), text: Rc::from(""),
                  parent: None, children: Vec::new(), js: None, listeners: Vec::new(),
                  handlers: Vec::new(), content: None, value: None, checked: None,
                  seq: 0, src_seq: 0, ce: CE_NONE, shadow: None, ui: 0, states: Vec::new() }
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

/// A change to the tree: the raw record for `MutationObserver`.
///
/// Recorded at the tree mutation sites, not derived by diffing snapshots:
/// a diff cannot say what changed, nor see that a node was removed and
/// reinserted.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MutKind { ChildList, Attributes, CharacterData }

#[derive(Clone)]
pub struct Mutation {
    pub kind: MutKind,
    /// For `ChildList` the parent, for the other two the node itself.
    pub target: u32,
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    pub prev: Option<u32>,
    pub next: Option<u32>,
    pub attr: Option<Rc<str>>,
    pub old: Option<Rc<str>>,
    /// Which observed nodes enclosed this change at the time it happened, as a
    /// bitmask over `Doc::observed`.
    ///
    /// Checking membership later would read the later tree: a node modified
    /// and then inserted would wrongly match, one modified and then removed
    /// would wrongly drop out.
    pub hits: u64,
}

/// Cap on the raw record.
///
/// A script building huge numbers of nodes while an observer is attached
/// must not exhaust memory. Overflow is reported, not swallowed silently.
pub const MAX_MUTATIONS: usize = 20_000;

pub struct Doc {
    pub nodes: Vec<DomNode>,
    pub doc: u32,
    pub html: Option<u32>,
    pub body: Option<u32>,
    pub head: Option<u32>,
    /// Has anything changed since the last write-back? Without it every click
    /// would rebuild the tree and relayout the page.
    pub dirty: bool,
    /// Does the page have any handlers? Until it does, layout need not record
    /// hit boxes.
    pub has_listeners: bool,
    /// The element with keyboard focus, set by `focus()`. The host reads it and
    /// sets its own focus accordingly (`forms.rs::FormState::focus`).
    pub focused: Option<u32>,
    /// Counts every change to the tree and is never reset.
    ///
    /// `dirty` answers "must I write back?" and is cleared on write-back, so it
    /// cannot serve a cache: after clearing, "unchanged since my state" and
    /// "changed twice since" look the same. A monotonic counter can tell.
    pub version: u32,
    /// The raw record for `MutationObserver`; empty while nobody observes.
    pub mutations: Vec<Mutation>,
    /// Is anybody observing? Without it every page would pay for a record
    /// nobody collects.
    pub observing: bool,
    /// Has the record overflowed? Reported on delivery.
    pub mut_overflow: bool,
    /// The observed nodes as `(node, subtree)`, in the bit order of
    /// `Mutation::hits`. The tree need not know who observes, only where.
    pub observed: Vec<(u32, bool)>,
    /// Is any custom element defined? Until one is, nothing is queued.
    pub ce_on: bool,
    /// Custom element reactions caused by tree changes, run by `ce_flush`
    /// before control returns to script.
    pub ce_queue: Vec<CeEvent>,
}

/// A tree change a custom element may have to react to.
#[derive(Clone)]
pub enum CeEvent {
    /// The subtree entered the document: upgrade, then `connectedCallback`.
    Inserted(u32),
    /// The subtree left the document: `disconnectedCallback`.
    Removed(u32),
    /// The subtree was created outside the document (`innerHTML` on a
    /// detached element, `cloneNode`): upgrade only.
    Created(u32),
    /// An attribute changed: element, name, old value, new value.
    Attr(u32, Rc<str>, Option<Rc<str>>, Option<Rc<str>>),
}

impl Doc {
    pub fn empty() -> Doc {
        let mut nodes = Vec::new();
        nodes.push(DomNode::new(DOCUMENT_NODE, "#document"));
        Doc { nodes, doc: 0, html: None, body: None, head: None,
              dirty: false, has_listeners: false, focused: None, version: 0,
              mutations: Vec::new(), observing: false, mut_overflow: false,
              observed: Vec::new(), ce_on: false, ce_queue: Vec::new() }
    }

    /// Is this node in the document, through shadow roots to their hosts?
    pub fn connected(&self, mut id: u32) -> bool {
        loop {
            if id == self.doc { return true }
            let n = &self.nodes[id as usize];
            match n.parent {
                Some(p) => id = p,
                None => match n.shadow {
                    Some(h) if &*n.tag == SHADOW_TAG => id = h,
                    _ => return false,
                },
            }
        }
    }

    fn ce_note_insert(&mut self, child: u32) {
        if self.ce_on && self.connected(child) { self.ce_queue.push(CeEvent::Inserted(child)); }
    }

    fn ce_note_remove(&mut self, id: u32) {
        if self.ce_on && self.nodes[id as usize].parent.is_some() && self.connected(id) {
            self.ce_queue.push(CeEvent::Removed(id));
        }
    }

    fn ce_note_attr(&mut self, id: u32, k: &str, old: Option<Rc<str>>, new: Option<Rc<str>>) {
        if self.ce_on && self.nodes[id as usize].ce == CE_CUSTOM {
            self.ce_queue.push(CeEvent::Attr(id, Rc::from(k), old, new));
        }
    }

    /// From beak's parsed tree. The original `seq` is not taken over; the
    /// arena assigns its own indices.
    pub fn from_dom(src: &crate::dom::Dom) -> Doc {
        let mut d = Doc::empty();
        let doc = d.doc;
        d.add_children(&src.root, doc);
        d.html = d.find_tag(d.doc, "html");
        d.body = d.find_tag(d.doc, "body");
        d.head = d.find_tag(d.doc, "head");
        // Building is not a mutation.
        d.dirty = false;
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
                    n.src_seq = el.seq;
                    for (k, v) in &el.attrs {
                        // An attribute handler is a handler like a registered one; without
                        // this the page would get no hit boxes and clicks would go nowhere.
                        if is_handler_attr(k) { self.has_listeners = true; }
                        n.attrs.push((Rc::from(k.as_str()), Rc::from(v.as_str())));
                    }
                    let id = self.push(n, parent);
                    self.add_children(el, id);
                }
            }
        }
    }

    pub fn push(&mut self, n: DomNode, parent: u32) -> u32 {
        self.touch();
        let id = self.nodes.len() as u32;
        self.nodes.push(n);
        self.nodes[id as usize].parent = Some(parent);
        self.nodes[parent as usize].children.push(id);
        id
    }

    /// A detached node without a parent (`createElement`).
    pub fn create(&mut self, kind: f64, tag: &str) -> u32 {
        self.touch();
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

    /// Which observed nodes enclose `node` now. Walks up the parent chain;
    /// the cost is the depth, not the node count.
    pub fn hits_for(&self, node: u32) -> u64 {
        let mut bits = 0u64;
        let mut cur = Some(node);
        let mut erste = true;
        while let Some(x) = cur {
            for (k, (t, sub)) in self.observed.iter().enumerate() {
                if k >= 64 { break }
                if *t == x && (erste || *sub) { bits |= 1u64 << k; }
            }
            erste = false;
            cur = self.nodes.get(x as usize).and_then(|n| n.parent);
        }
        bits
    }

    /// Record a change, only if somebody observes and it concerns them.
    pub fn record(&mut self, mut m: Mutation) {
        if !self.observing { return }
        m.hits = self.hits_for(m.target);
        if m.hits == 0 { return }
        if self.mutations.len() >= MAX_MUTATIONS { self.mut_overflow = true; return }
        self.mutations.push(m);
    }

    /// The siblings of a node as they stand now. A record names them as they
    /// were at the time of the change; afterwards they are stale.
    fn siblings(&self, parent: u32, at: usize) -> (Option<u32>, Option<u32>) {
        let k = &self.nodes[parent as usize].children;
        (if at > 0 { k.get(at - 1).copied() } else { None }, k.get(at + 1).copied())
    }

    /// Detach from the old parent. Must run before every insert, otherwise a
    /// node sits in two child lists.
    pub fn detach(&mut self, id: u32) {
        self.touch();
        self.ce_note_remove(id);
        if let Some(p) = self.nodes[id as usize].parent {
            let at = self.nodes[p as usize].children.iter().position(|&c| c == id);
            if self.observing {
                if let Some(at) = at {
                    let (prev, next) = self.siblings(p, at);
                    self.record(Mutation { kind: MutKind::ChildList, target: p,
                        added: Vec::new(), removed: alloc::vec![id],
                        prev, next, attr: None, old: None, hits: 0 });
                }
            }
            self.nodes[p as usize].children.retain(|&c| c != id);
        }
        self.nodes[id as usize].parent = None;
    }

    pub fn append(&mut self, parent: u32, child: u32) {
        self.touch();
        // Moving a node is a removal and an insertion, and both are reported
        // (DOM §4.2.3); an observer that saw only the insertion would have the
        // node in the tree twice.
        self.detach(child);
        self.nodes[child as usize].parent = Some(parent);
        self.nodes[parent as usize].children.push(child);
        if self.observing {
            let n = self.nodes[parent as usize].children.len();
            let prev = if n >= 2 { self.nodes[parent as usize].children.get(n - 2).copied() } else { None };
            self.record(Mutation { kind: MutKind::ChildList, target: parent,
                added: alloc::vec![child], removed: Vec::new(),
                prev, next: None, attr: None, old: None, hits: 0 });
        }
        self.ce_note_insert(child);
    }

    /// Insert; a fragment hands over its children instead of being inserted
    /// itself (DOM §4.2.3). `ul.appendChild(tpl.content.cloneNode(true))`
    /// must not put a `#fragment` element into the tree.
    pub fn insert_maybe_fragment(&mut self, parent: u32, child: u32, before: Option<u32>) {
        if &*self.nodes[child as usize].tag == "#fragment" {
            for k in self.nodes[child as usize].children.clone() {
                self.insert_before(parent, k, before);
            }
            return;
        }
        self.insert_before(parent, child, before);
    }

    pub fn insert_before(&mut self, parent: u32, child: u32, before: Option<u32>) {
        self.touch();
        self.detach(child);
        self.nodes[child as usize].parent = Some(parent);
        let at = match before {
            Some(b) => self.nodes[parent as usize].children.iter().position(|&c| c == b)
                        .unwrap_or(self.nodes[parent as usize].children.len()),
            None => self.nodes[parent as usize].children.len(),
        };
        self.nodes[parent as usize].children.insert(at, child);
        if self.observing {
            let (prev, next) = self.siblings(parent, at);
            self.record(Mutation { kind: MutKind::ChildList, target: parent,
                added: alloc::vec![child], removed: Vec::new(),
                prev, next, attr: None, old: None, hits: 0 });
        }
        self.ce_note_insert(child);
    }

    /// Set an attribute. The single path, so the change is recorded once.
    pub fn set_attr_at(&mut self, id: u32, k: &str, v: &str) {
        self.touch();
        let old = self.nodes[id as usize].attr(k).cloned();
        if self.observing {
            self.record(Mutation { kind: MutKind::Attributes, target: id,
                added: Vec::new(), removed: Vec::new(), prev: None, next: None,
                attr: Some(Rc::from(k)), old: old.clone(), hits: 0 });
        }
        self.nodes[id as usize].set_attr(k, v);
        self.ce_note_attr(id, k, old, Some(Rc::from(v)));
    }

    /// Remove an attribute. If it is absent, that is not a change and nothing
    /// is recorded.
    pub fn remove_attr_at(&mut self, id: u32, k: &str) {
        self.touch();
        let old = self.nodes[id as usize].attr(k).cloned();
        if old.is_none() { return }
        if self.observing {
            self.record(Mutation { kind: MutKind::Attributes, target: id,
                added: Vec::new(), removed: Vec::new(), prev: None, next: None,
                attr: Some(Rc::from(k)), old: old.clone(), hits: 0 });
        }
        self.nodes[id as usize].attrs.retain(|(n, _)| &**n != k);
        self.ce_note_attr(id, k, old, None);
    }

    /// Set the text of a text/comment node (`data`, `nodeValue`). Not for
    /// freshly built nodes: those have no old value and no observer.
    pub fn set_text(&mut self, id: u32, v: Rc<str>) {
        self.touch();
        if self.observing {
            let old = Some(self.nodes[id as usize].text.clone());
            self.record(Mutation { kind: MutKind::CharacterData, target: id,
                added: Vec::new(), removed: Vec::new(), prev: None, next: None,
                attr: None, old, hits: 0 });
        }
        self.nodes[id as usize].text = v;
    }

    /// The concatenated text of a subtree.
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

    /// All elements in document order from `from`.
    pub fn descendants(&self, from: u32, out: &mut Vec<u32>) {
        for &c in &self.nodes[from as usize].children {
            if self.nodes[c as usize].kind == ELEMENT_NODE { out.push(c); }
            self.descendants(c, out);
        }
    }
}

impl Doc {
    /// Parse HTML as a fragment and insert it under `parent`; the path of
    /// `innerHTML =` and `insertAdjacentHTML`. Uses beak's own parser.
    ///
    /// `dom::parse` always builds a whole document (`<li>x</li>` becomes
    /// `<html><body><li>x`), so the `<html>`/`<body>` wrapper is stripped;
    /// otherwise `el.children[0]` would be the wrapper, not the first element.
    pub fn parse_into(&mut self, parent: u32, html: &str, at: Option<usize>) -> Vec<u32> {
        let frag = crate::dom::parse(html);
        let mut made = Vec::new();
        for c in fragment_nodes(&frag.root) {
            if let Some(id) = self.from_src_node(c) { made.push(id); }
        }
        let idx = at.unwrap_or(self.nodes[parent as usize].children.len());
        for (k, id) in made.iter().enumerate() {
            self.nodes[*id as usize].parent = Some(parent);
            let pos = (idx + k).min(self.nodes[parent as usize].children.len());
            self.nodes[parent as usize].children.insert(pos, *id);
            // Elements the fragment parser creates are upgraded even when
            // detached (HTML §4.13.3, synchronous flag unset).
            if self.ce_on {
                let ev = if self.connected(*id) { CeEvent::Inserted(*id) } else { CeEvent::Created(*id) };
                self.ce_queue.push(ev);
            }
        }
        made
    }

    /// Parse a whole document; the path of `DOMParser.parseFromString`.
    /// Unlike `parse_into` the `<html>`/`<head>`/`<body>` frame stays: here it
    /// is the result.
    ///
    /// Detached like `createHTMLDocument`: same node store, but no parent, so
    /// neither layout nor host sees it.
    pub fn parse_document(&mut self, src: &str, html_kind: bool) -> u32 {
        let parsed = crate::dom::parse(src);
        let doc = self.create(DOCUMENT_NODE, "#document");
        if !html_kind {
            // XML and SVG have no `<body>` frame: the root of the text is the root
            // of the document.
            for c in fragment_nodes(&parsed.root) {
                if let Some(id) = self.from_src_node(c) { self.append(doc, id); }
            }
            return doc;
        }
        let root = parsed.root.children.iter().find_map(|c| match c {
            crate::dom::Node::Element(e) if &*e.tag == "html" => Some(c),
            _ => None,
        });
        let html = match root.and_then(|c| self.from_src_node(c)) {
            Some(id) => id,
            None => self.create(ELEMENT_NODE, "html"),
        };
        self.append(doc, html);
        // The parser omits an empty `<head>`, the spec does not; without this
        // `doc.head` would be `null` for a document whose head is merely empty.
        for (k, tag) in ["head", "body"].iter().enumerate() {
            if !self.nodes[html as usize].children.iter()
                .any(|&c| &*self.nodes[c as usize].tag == *tag) {
                let el = self.create(ELEMENT_NODE, tag);
                self.nodes[el as usize].parent = Some(html);
                let at = k.min(self.nodes[html as usize].children.len());
                self.nodes[html as usize].children.insert(at, el);
            }
        }
        doc
    }

    /// Put a parsed subtree into the arena, still without a parent.
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
                    if is_handler_attr(k) { self.has_listeners = true; }
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

    /// A node as HTML, for `innerHTML`/`outerHTML`. Escaping is required: an
    /// unescaped `<` in text turns content into markup.
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

    /// Detach all children of a node (for `innerHTML =`).
    pub fn clear_children(&mut self, id: u32) {
        self.touch();
        let old: Vec<u32> = self.nodes[id as usize].children.clone();
        for c in &old { self.ce_note_remove(*c); }
        for c in &old { self.nodes[*c as usize].parent = None; }
        self.nodes[id as usize].children.clear();
        // `innerHTML = "…"` clears before rebuilding. Without this record an
        // observer would see only the new nodes, never that the old ones left,
        // and component libraries clean up on exactly that.
        if self.observing && !old.is_empty() {
            self.record(Mutation { kind: MutKind::ChildList, target: id,
                added: Vec::new(), removed: old, prev: None, next: None,
                attr: None, old: None, hits: 0 });
        }
    }

    /// Copy a subtree (`cloneNode`).
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
                let cc = self.clone_node_inner(c);
                self.nodes[cc as usize].parent = Some(new);
                self.nodes[new as usize].children.push(cc);
            }
        }
        if self.ce_on { self.ce_queue.push(CeEvent::Created(new)); }
        new
    }

    fn clone_node_inner(&mut self, id: u32) -> u32 {
        let (kind, tag, attrs, text, kids) = {
            let n = &self.nodes[id as usize];
            (n.kind, n.tag.clone(), n.attrs.clone(), n.text.clone(), n.children.clone())
        };
        let new = self.create(kind, &tag);
        self.nodes[new as usize].attrs = attrs;
        self.nodes[new as usize].text = text;
        for c in kids {
            let cc = self.clone_node_inner(c);
            self.nodes[cc as usize].parent = Some(new);
            self.nodes[new as usize].children.push(cc);
        }
        new
    }

    /// Mark a change to the tree: `dirty` for write-back, `version` for
    /// anyone caching on it.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    /// The live tree as the cascade needs it, without changing anything.
    ///
    /// Unlike `to_dom` this assigns no new `seq`: `to_dom` re-links layout,
    /// and calling it to answer one question would invalidate every box of
    /// the current layout. The id is the arena index instead, which never
    /// changes; `find_path` finds the element by the same number the caller
    /// holds.
    pub fn live_dom(&self) -> crate::dom::Dom {
        let mut root = crate::dom::Element::bare("#root".into(), 0);
        for c in &self.nodes[self.doc as usize].children {
            if let Some(n) = self.live_node(*c) { root.children.push(n); }
        }
        root.index_attrs();
        crate::dom::Dom { root }
    }

    fn live_node(&self, id: u32) -> Option<crate::dom::Node> {
        let n = &self.nodes[id as usize];
        if n.kind == TEXT_NODE { return Some(crate::dom::Node::Text(n.text.to_string())); }
        if n.kind != ELEMENT_NODE { return None; }
        let mut e = crate::dom::Element::bare(n.tag.to_string(), id);
        e.defined = ce_defined(n);
        for (k, v) in &n.attrs { e.attrs.push((k.to_string(), v.to_string())); }
        // Must run after the attributes; see `to_node`.
        e.index_attrs();
        for c in &n.children {
            if let Some(x) = self.live_node(*c) { e.children.push(x); }
        }
        Some(crate::dom::Node::Element(e))
    }

    /// Write the arena back into beak's tree; layout, cascade and forms read
    /// `dom::Dom`, so DOM changes become visible only through this. A full
    /// rebuild per script run, not per change.
    ///
    /// `seq` is reassigned in document order, as the parser does, so the
    /// identity form state hangs on stays stable for untouched nodes.
    pub fn to_dom(&mut self) -> crate::dom::Dom {
        let mut seq = 0u32;
        let mut root = crate::dom::Element::bare("#root".into(), seq);
        for c in self.nodes[self.doc as usize].children.clone() {
            if let Some(n) = self.to_node(c, &mut seq) { root.children.push(n); }
        }
        root.index_attrs();
        self.dirty = false;
        crate::dom::Dom { root }
    }

    /// The arena node for a `seq` from layout.
    pub fn by_seq(&self, seq: u32) -> Option<u32> {
        self.nodes.iter().position(|n| n.kind == ELEMENT_NODE && n.seq == seq).map(|i| i as u32)
    }

    fn to_node(&mut self, id: u32, seq: &mut u32) -> Option<crate::dom::Node> {
        let (kind, tag, attrs, text, kids) = {
            let n = &self.nodes[id as usize];
            (n.kind, n.tag.clone(), n.attrs.clone(), n.text.clone(), n.children.clone())
        };
        if kind == TEXT_NODE { return Some(crate::dom::Node::Text(text.to_string())); }
        if kind != ELEMENT_NODE { return None; }
        *seq += 1;
        // The bridge: the same number now stands here and in layout.
        self.nodes[id as usize].seq = *seq;
        let mut e = crate::dom::Element::bare(tag.to_string(), *seq);
        e.defined = ce_defined(&self.nodes[id as usize]);
        for (k, v) in &attrs { e.attrs.push((k.to_string(), v.to_string())); }
        // Must run after the attributes, otherwise classes, id and the bloom
        // filter are empty and no selector matches.
        e.index_attrs();
        for c in kids {
            if let Some(x) = self.to_node(c, seq) { e.children.push(x); }
        }
        Some(crate::dom::Node::Element(e))
    }
}

/// A page script: either its text or the URL it lives at.
pub enum ScriptRef {
    /// Source or URL, whether `type="module"` is set, and the `<script>` node.
    /// The module flag is recorded here, not guessed later: a module without
    /// `import` also parses as a script, but would get the wrong scope and
    /// `this`. The node is `document.currentScript`.
    Inline(String, bool, u32),
    External(String, bool, u32),
}

/// Is `t` the root element (`<html>`)?
fn is_root_element(i: &mut Interp, t: &Value) -> bool {
    let Ok(id) = node_of(i, t) else { return false };
    i.doc.as_ref().and_then(|d| d.html) == Some(id)
}

/// Does this element scroll the page? The root element and `<body>` do;
/// no other element has its own scroll box in beak.
///
/// `document.scrollingElement` names the same element; pages read it and
/// then write its `scrollTop`.
fn is_scrolling_root(i: &mut Interp, t: &Value) -> bool {
    let Ok(id) = node_of(i, t) else { return false };
    let Some(d) = &i.doc else { return false };
    d.html == Some(id) || d.body == Some(id)
}

/// The scrollable overflow of an ordinary element: its padding box united
/// with the border boxes of all descendants. That is what `scrollHeight`
/// asks: does the content fit the box?
fn scroll_area(i: &Interp, t: &Value) -> Option<(f64, f64)> {
    let id = node_of_ref(i, t).ok()?;
    let g = i.geometry.as_ref()?;
    let d = i.doc.as_ref()?;
    let (own, borders, _) = node_box(i, id)?;
    // The padding box is the floor: `scrollHeight` is never smaller than
    // `clientHeight`. Measured from the padding edge, so only the border is
    // subtracted; only per-axis sums are stored, so half is the estimate for
    // one side. Exact for symmetric borders.
    let (bl, bt) = (borders.0 / 2.0, borders.1 / 2.0);
    let mut w = (own.2 - borders.0).max(0.0);
    let mut h = (own.3 - borders.1).max(0.0);
    let (ox, oy) = (own.0 + bl, own.1 + bt);
    let mut kids = Vec::new();
    d.descendants(id, &mut kids);
    for k in kids {
        let Some(seq) = node_seq(i, k) else { continue };
        for b in g.boxes.iter().filter(|b| b.seq == seq) {
            let bx = (b.x - g.scroll.0) as f64;
            let by = (b.y - g.scroll.1) as f64;
            w = w.max(bx + b.w as f64 - ox);
            h = h.max(by + b.h as f64 - oy);
        }
    }
    Some((w, h))
}

/// The arguments of `scrollTo`/`scrollBy`: two numbers or an object with
/// `left`/`top`. A missing axis is `None` and left alone:
/// `scrollTo({ top: 0 })` must not jump horizontally.
fn scroll_args(i: &mut Interp, a: &[Value]) -> C<(Option<f64>, Option<f64>)> {
    match a.first() {
        Some(o @ Value::Obj(_)) => {
            let l = match i.get(o, "left")? { Value::Undefined => None, v => Some(i.to_number(&v)?) };
            let t = match i.get(o, "top")? { Value::Undefined => None, v => Some(i.to_number(&v)?) };
            Ok((l, t))
        }
        _ => {
            let x = match a.first() { None | Some(Value::Undefined) => None,
                                      Some(v) => Some(i.to_number(v)?) };
            let y = match a.get(1) { None | Some(Value::Undefined) => None,
                                     Some(v) => Some(i.to_number(v)?) };
            Ok((x, y))
        }
    }
}

/// A viewport dimension as `set_viewport` stored it.
fn viewport_num(i: &mut Interp, key: &str) -> Value {
    let g = Value::Obj(i.realm.global.clone());
    match i.get(&g, key) { Ok(v @ Value::Num(_)) => v, _ => Value::Num(0.0) }
}

/// The same as a number.
fn viewport_f(i: &mut Interp, key: &str) -> f64 {
    match viewport_num(i, key) { Value::Num(n) => n, _ => 0.0 }
}

/// All page scripts, inline and external, in document order.
///
/// beak runs them in source order after the document is complete, so all
/// scripts behave like `defer`; a parser-blocking script mid-body is not
/// modelled (see `doc_write` for the one place it matters).
pub fn page_scripts(d: &Doc) -> Vec<ScriptRef> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    let mut out = Vec::new();
    for id in all {
        let n = &d.nodes[id as usize];
        if &*n.tag != "script" { continue; }
        if !script_type_is_js(n) { continue; }
        let module = n.attr("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("module"));
        // `nomodule` means "only for a browser without modules" (HTML §4.12.1).
        // beak has modules, so this branch is skipped; running both would let two
        // versions of one library fight over the same global name, and a
        // polyfill bundle may replace built-ins. On a module script the
        // attribute is ignored, per spec.
        if !module && n.attr("nomodule").is_some() { continue; }
        match n.attr("src") {
            Some(src) if !src.trim().is_empty() =>
                out.push(ScriptRef::External(src.to_string(), module, id)),
            _ => {
                let text = d.text_of(id);
                if !text.trim().is_empty() { out.push(ScriptRef::Inline(text, module, id)); }
            }
        }
    }
    out
}

/// A `type` that does not mean JavaScript (`application/json`,
/// `text/template`) is payload, not a program.
fn script_type_is_js(n: &DomNode) -> bool {
    match n.attr("type") {
        None => true,
        Some(t) => {
            let t = t.to_ascii_lowercase();
            t.is_empty() || t.contains("javascript") || t == "module" || t == "text/ecmascript"
        }
    }
}

/// The text of all `<script>` elements without `src`, in document order.
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

// ── Selectors ───────────────────────────────────────────────────────────────
//
// `js::qsel` parses and matches on the arena. An invalid selector throws
// `SyntaxError` (DOM §4.2.6 "scope-match a selectors string").

/// Parse a selector for the DOM API, or the exception to throw.
fn parse_selector(i: &mut Interp, s: &str) -> C<super::qsel::SelList> {
    super::qsel::parse(s).map_err(|e| dom_exc(i, "SyntaxError",
        &alloc::format!("'{s}' is not a valid selector: {e}")))
}

/// The fragment of the document URL, for `:target`.
fn url_fragment(i: &Interp) -> String {
    i.loc_href.split_once('#').map(|(_, f)| f.to_string()).unwrap_or_default()
}

/// Does `id` match `sel`, with `scope` as `:scope`?
fn sel_matches(i: &Interp, sel: &super::qsel::SelList, id: u32, scope: Option<u32>) -> bool {
    let Some(d) = &i.doc else { return false };
    let target = url_fragment(i);
    let cx = super::qsel::Ctx { d, scope, target: &target };
    sel.matches(&cx, id)
}

/// The elements below `from` that match, in tree order.
fn sel_query(i: &Interp, sel: &super::qsel::SelList, from: u32, first_only: bool) -> Vec<u32> {
    let Some(d) = &i.doc else { return Vec::new() };
    let target = url_fragment(i);
    // On the document `:scope` is the root element; on an element, itself.
    let scope = if from == d.doc { d.html } else { Some(from) };
    let cx = super::qsel::Ctx { d, scope, target: &target };
    super::qsel::query_all(&cx, from, sel, first_only)
}

/// Selector matching for callers without an interpreter. An invalid
/// selector matches nothing.
pub fn selector_match(d: &Doc, id: u32, sel: &str) -> bool {
    let Ok(l) = super::qsel::parse(sel) else { return false };
    let cx = super::qsel::Ctx { d, scope: Some(id), target: "" };
    l.matches(&cx, id)
}

pub fn query(d: &Doc, from: u32, sel: &str, all: bool) -> Vec<u32> {
    let Ok(l) = super::qsel::parse(sel) else { return Vec::new() };
    let scope = if from == d.doc { d.html } else { Some(from) };
    let cx = super::qsel::Ctx { d, scope, target: "" };
    super::qsel::query_all(&cx, from, &l, !all)
}

// ── The JS side ─────────────────────────────────────────────────────────────

use super::interp::C;

/// The index in the wrapper object. Non-enumerable and non-configurable, so
/// a script iterating an element's properties does not see it.
const SLOT: &str = "__node";

pub fn node_of(i: &mut Interp, v: &Value) -> C<u32> {
    match i.get(v, SLOT)? {
        Value::Num(n) if n >= 0.0 => Ok(n as u32),
        _ => i.type_err("not a DOM node"),
    }
}

/// Like `node_of`, but `window` counts as the document. The window receives
/// bubbling events last, and the root node sits exactly there in every
/// propagation path, so it is the right target, not an approximation.
fn target_node(i: &mut Interp, v: &Value) -> C<u32> {
    // A missing `this` means the global object, not "no target" (WebIDL
    // §3.7.4): if `this` is null or undefined the global object takes its
    // place, and only then is the interface checked. `window` satisfies
    // `EventTarget`, which makes a bare `addEventListener("resize", f)` work.
    //
    // `node_of` does not get this rule: `window` is not a `Node`, so a bare
    // `appendChild(x)` must still throw.
    if matches!(v, Value::Undefined | Value::Null) {
        return match &i.doc { Some(d) => Ok(d.doc), None => i.type_err("no document") };
    }
    if let Value::Obj(o) = v {
        if Rc::ptr_eq(o, &i.realm.global) {
            return match &i.doc { Some(d) => Ok(d.doc), None => i.type_err("no document") };
        }
    }
    node_of(i, v)
}

/// Which part of a document is wanted.
#[derive(Clone, Copy, PartialEq)]
enum DocPart { Root, Head, Body }

/// `documentElement` / `head` / `body`, relative to this document node.
///
/// For the main document the answer is cached in `Doc`; for any other
/// (`implementation.createHTMLDocument`) the tree is walked. The cached
/// path stays correct when a page rebuilds its body.
fn doc_part(i: &mut Interp, this: &Value, part: DocPart) -> C<Option<u32>> {
    let id = node_of(i, this)?;
    let Some(d) = &i.doc else { return Ok(None) };
    if id == d.doc {
        return Ok(match part { DocPart::Root => d.html, DocPart::Head => d.head, DocPart::Body => d.body });
    }
    // The root is the first element under the document node.
    let root = d.nodes.get(id as usize)
        .and_then(|n| n.children.iter().copied()
            .find(|c| d.nodes[*c as usize].kind == ELEMENT_NODE));
    let Some(root) = root else { return Ok(None) };
    if part == DocPart::Root { return Ok(Some(root)) }
    let want = if part == DocPart::Head { "head" } else { "body" };
    Ok(d.nodes[root as usize].children.iter().copied()
        .find(|c| &*d.nodes[*c as usize].tag == want))
}

// ── ResizeObserver + IntersectionObserver ───────────────────────────────
//
// Both depend on geometry after layout. The host provides it via
// `Interp::set_geometry`, and that is where the observers are evaluated,
// not in a timer guessing when something might have moved.
//
// Measured at observation time, delivered later. An entry holds the boxes
// as they were when observed; recomputing at delivery would hand the
// callback numbers from another round, and another observer's callback,
// which may change the tree, runs in between.

/// Which box a `ResizeObserver` reports.
#[derive(Clone, Copy, PartialEq)]
pub enum BoxKind { Content, Border }

/// One `ResizeObserver` registration.
pub struct ResizeReg {
    /// The node, not its layout `seq`: layout reassigns `seq` on every run, a
    /// registration outlives every layout.
    pub target: u32,
    pub kind: BoxKind,
    /// Last reported size. `None` means never; the first run always reports,
    /// as the spec requires: `observe` delivers the current size, not only
    /// the next change.
    pub last: Option<(f64, f64)>,
}

/// What a `ResizeObserverEntry` says, computed at observation time.
pub struct ResizeEntry {
    pub target: u32,
    /// Content box: width, height.
    pub content: (f64, f64),
    /// Border box: width, height.
    pub border: (f64, f64),
}

pub struct ResizeObs {
    pub js: Gc,
    pub cb: Value,
    pub regs: Vec<ResizeReg>,
    pub queue: Vec<ResizeEntry>,
}

/// One `IntersectionObserver` registration.
pub struct InterReg {
    pub target: u32,
    /// Threshold index from last time: how many thresholds the ratio reached.
    /// Reported when this number changes, not on every pixel, so scrolling is
    /// not a storm of callbacks. `-1` means never reported.
    pub band: i32,
}

/// What an `IntersectionObserverEntry` says.
pub struct InterEntry {
    pub target: u32,
    pub ratio: f64,
    pub hit: bool,
    pub root: Option<(f64, f64, f64, f64)>,
    pub bounds: (f64, f64, f64, f64),
    pub inter: (f64, f64, f64, f64),
    pub time: f64,
}

pub struct InterObs {
    pub js: Gc,
    pub cb: Value,
    /// `None` means the viewport. Another element as root is allowed and
    /// handled the same way; its box is then the clip.
    pub root: Option<u32>,
    /// The unparsed `rootMargin` text, so the getter can return it without
    /// reconstructing text from four numbers.
    pub margin_src: Rc<str>,
    /// `rootMargin` in pixels, top/right/bottom/left. Percentages are resolved
    /// at registration; the spec allows both.
    pub margin: (f64, f64, f64, f64),
    pub thresholds: Vec<f64>,
    pub regs: Vec<InterReg>,
    pub queue: Vec<InterEntry>,
}

/// The layout `seq` of a node, like `layout_seq` but from the id. Observers
/// know their targets as nodes.
fn node_seq(i: &Interp, id: u32) -> Option<u32> {
    let n = i.doc.as_ref()?.nodes.get(id as usize)?;
    Some(if n.seq != 0 { n.seq } else { n.src_seq }).filter(|s| *s != 0)
}

/// The border box of a node in viewport coordinates (the union of its
/// fragments, as `getBoundingClientRect`), plus the border and padding sums
/// of the first fragment, kept separate.
///
/// Separate because `contentRect` subtracts both, while `scrollHeight`
/// measures from the padding edge and subtracts only the border.
///
/// Same computation as `elem_rect`, without going through a JS value.
fn node_box(i: &Interp, id: u32) -> Option<((f64, f64, f64, f64), (f64, f64), (f64, f64))> {
    let g = i.geometry.as_ref()?;
    let seq = node_seq(i, id)?;
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    let mut borders = (0.0, 0.0);
    let mut pads = (0.0, 0.0);
    for b in g.boxes.iter().filter(|b| b.seq == seq) {
        if acc.is_none() {
            borders = (b.bx as f64, b.by as f64);
            pads = (b.px as f64, b.py as f64);
        }
        acc = Some(match acc {
            None => (b.x, b.y, b.x + b.w, b.y + b.h),
            Some((x0, y0, x1, y1)) =>
                (x0.min(b.x), y0.min(b.y), x1.max(b.x + b.w), y1.max(b.y + b.h)),
        });
    }
    let (x0, y0, x1, y1) = acc?;
    Some((((x0 - g.scroll.0) as f64, (y0 - g.scroll.1) as f64,
           (x1 - x0) as f64, (y1 - y0) as f64), borders, pads))
}

/// `rootMargin`: one to four CSS lengths, top/right/bottom/left as for
/// `margin`. Percentages refer to the clip size: horizontal to the width,
/// vertical to the height.
fn parse_root_margin(s: &str, vw: f64, vh: f64) -> (f64, f64, f64, f64) {
    let one = |t: &str, basis: f64| -> f64 {
        let t = t.trim();
        if let Some(n) = t.strip_suffix('%') {
            n.trim().parse::<f64>().map(|v| v / 100.0 * basis).unwrap_or(0.0)
        } else if let Some(n) = t.strip_suffix("px") {
            n.trim().parse::<f64>().unwrap_or(0.0)
        } else {
            // A bare number is not a length (only `0` would be), nor is an unknown
            // unit. Both become 0 rather than guessed.
            t.parse::<f64>().ok().filter(|v| *v == 0.0).unwrap_or(0.0)
        }
    };
    let parts: Vec<&str> = s.split_whitespace().collect();
    match parts.len() {
        0 => (0.0, 0.0, 0.0, 0.0),
        1 => { let t = one(parts[0], vh); let r = one(parts[0], vw); (t, r, t, r) }
        2 => (one(parts[0], vh), one(parts[1], vw), one(parts[0], vh), one(parts[1], vw)),
        3 => (one(parts[0], vh), one(parts[1], vw), one(parts[2], vh), one(parts[1], vw)),
        _ => (one(parts[0], vh), one(parts[1], vw), one(parts[2], vh), one(parts[3], vw)),
    }
}

/// Evaluate both observer kinds. Called from `Interp::set_geometry`, the
/// moment the host reports the current layout.
pub fn eval_box_observers(i: &mut Interp) {
    // ── ResizeObserver ───────────────────────────────────────────────────
    for n in 0..i.resize_obs.len() {
        for r in 0..i.resize_obs[n].regs.len() {
            let (id, kind, last) = {
                let reg = &i.resize_obs[n].regs[r];
                (reg.target, reg.kind, reg.last)
            };
            let Some(((_, _, w, h), (bx, by), (px, py))) = node_box(i, id) else { continue };
            let content = ((w - bx - px).max(0.0), (h - by - py).max(0.0));
            let border = (w, h);
            let seen = match kind { BoxKind::Border => border, BoxKind::Content => content };
            // An observer reports changes, and the first time the current state.
            if last != Some(seen) {
                i.resize_obs[n].regs[r].last = Some(seen);
                i.resize_obs[n].queue.retain(|e| e.target != id);
                i.resize_obs[n].queue.push(ResizeEntry { target: id, content, border });
            }
        }
    }

    // ── IntersectionObserver ─────────────────────────────────────────────
    let (vw, vh) = i.viewport;
    let now = i.now_ms();
    for n in 0..i.inter_obs.len() {
        // The clip: the viewport or the root's box, in both cases grown by
        // `rootMargin`.
        let root = match i.inter_obs[n].root {
            None => (0.0, 0.0, vw, vh),
            Some(id) => match node_box(i, id) { Some((r, _, _)) => r, None => continue },
        };
        let (mt, mr, mb, ml) = i.inter_obs[n].margin;
        let rx0 = root.0 - ml;
        let ry0 = root.1 - mt;
        let rx1 = root.0 + root.2 + mr;
        let ry1 = root.1 + root.3 + mb;
        let rroot = (rx0, ry0, rx1 - rx0, ry1 - ry0);
        for r in 0..i.inter_obs[n].regs.len() {
            let (id, band) = {
                let reg = &i.inter_obs[n].regs[r];
                (reg.target, reg.band)
            };
            let Some(((tx, ty, tw, th), _, _)) = node_box(i, id) else { continue };
            let ix0 = tx.max(rx0);
            let iy0 = ty.max(ry0);
            let ix1 = (tx + tw).min(rx1);
            let iy1 = (ty + th).min(ry1);
            let iw = (ix1 - ix0).max(0.0);
            let ih = (iy1 - iy0).max(0.0);
            let area = tw * th;
            let overlap = iw > 0.0 && ih > 0.0;
            // A box without area (an empty line, a wrapped zero-width inline box)
            // has no ratio; it intersects when it lies inside the clip. Otherwise
            // the answer would be 0/0.
            let inside = tx >= rx0 && tx <= rx1 && ty >= ry0 && ty <= ry1;
            let (hit, ratio) = if area > 0.0 {
                (overlap, if overlap { iw * ih / area } else { 0.0 })
            } else {
                (inside, if inside { 1.0 } else { 0.0 })
            };
            // How many thresholds are reached. Threshold 0 only counts as reached
            // when there is any intersection at all, otherwise every element would
            // start "above 0".
            let hits = i.inter_obs[n].thresholds.iter()
                .filter(|t| if **t <= 0.0 { hit } else { ratio >= **t - 1e-9 })
                .count() as i32;
            if hits != band {
                i.inter_obs[n].regs[r].band = hits;
                i.inter_obs[n].queue.retain(|e| e.target != id);
                i.inter_obs[n].queue.push(InterEntry {
                    target: id, ratio, hit,
                    root: Some(rroot),
                    bounds: (tx, ty, tw, th),
                    inter: if hit { (ix0, iy0, iw, ih) } else { (0.0, 0.0, 0.0, 0.0) },
                    time: now,
                });
            }
        }
    }
}

/// Build the JS object from a `ResizeEntry`.
fn build_resize_entry(i: &mut Interp, e: &ResizeEntry) -> Value {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    let target = wrap(i, e.target);
    // `contentRect` is in the padding box: x/y are the left and top padding.
    // Only sums are stored, so half is used; exact for symmetric padding.
    let rect = rect_obj(i, Some((0.0, 0.0, e.content.0, e.content.1)));
    let size = |i: &mut Interp, (w, h): (f64, f64)| -> Value {
        let s = new_obj(Some(i.realm.object_proto.clone()));
        s.borrow_mut().define("inlineSize", Prop::data(Value::Num(w)));
        s.borrow_mut().define("blockSize", Prop::data(Value::Num(h)));
        // A list, because a fragmented box could have several sizes. There is
        // always exactly one here, but page code writes
        // `entry.contentBoxSize[0].inlineSize`.
        i.new_array(alloc::vec![Value::Obj(s)])
    };
    let content_size = size(i, e.content);
    let border_size = size(i, e.border);
    {
        let mut b = o.borrow_mut();
        b.define("target", Prop::data(target));
        b.define("contentRect", Prop::data(Value::Obj(rect)));
        b.define("contentBoxSize", Prop::data(content_size));
        b.define("borderBoxSize", Prop::data(border_size));
        // No device-pixel grid is known, so the list is empty rather than
        // invented.
        b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("ResizeObserverEntry")));
    }
    let empty = i.new_array(Vec::new());
    o.borrow_mut().define("devicePixelContentBoxSize", Prop::data(empty));
    Value::Obj(o)
}

/// Build the JS object from an `InterEntry`.
fn build_inter_entry(i: &mut Interp, e: &InterEntry) -> Value {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    let target = wrap(i, e.target);
    let bounds = rect_obj(i, Some(e.bounds));
    let inter = rect_obj(i, Some(e.inter));
    let root = match e.root { Some(r) => Value::Obj(rect_obj(i, Some(r))), None => Value::Null };
    let mut b = o.borrow_mut();
    b.define("target", Prop::data(target));
    b.define("time", Prop::data(Value::Num(e.time)));
    b.define("boundingClientRect", Prop::data(Value::Obj(bounds)));
    b.define("intersectionRect", Prop::data(Value::Obj(inter)));
    b.define("rootBounds", Prop::data(root));
    b.define("intersectionRatio", Prop::data(Value::Num(e.ratio)));
    b.define("isIntersecting", Prop::data(Value::Bool(e.hit)));
    b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("IntersectionObserverEntry")));
    drop(b);
    Value::Obj(o)
}

/// The checkpoint: observers with queued entries are called. Runs next to
/// `deliver_mutations` and for the same reason: after every entry point,
/// not in a timer.
///
/// Returns true if a callback ran.
pub fn deliver_box_observers(i: &mut Interp) -> bool {
    let mut ran = false;
    for n in 0..i.resize_obs.len() {
        if i.resize_obs[n].queue.is_empty() { continue }
        let q = core::mem::take(&mut i.resize_obs[n].queue);
        let cb = i.resize_obs[n].cb.clone();
        let this = Value::Obj(i.resize_obs[n].js.clone());
        let vals: Vec<Value> = q.iter().map(|e| build_resize_entry(i, e)).collect();
        let arr = i.new_array(vals);
        ran = true;
        if let Err(e) = i.call(&cb, this.clone(), &[arr, this]) {
            let msg = super::modules::describe(i, e);
            i.console_push(alloc::format!("error: ResizeObserver-Rueckruf: {msg}"));
        }
    }
    for n in 0..i.inter_obs.len() {
        if i.inter_obs[n].queue.is_empty() { continue }
        let q = core::mem::take(&mut i.inter_obs[n].queue);
        let cb = i.inter_obs[n].cb.clone();
        let this = Value::Obj(i.inter_obs[n].js.clone());
        let vals: Vec<Value> = q.iter().map(|e| build_inter_entry(i, e)).collect();
        let arr = i.new_array(vals);
        ran = true;
        if let Err(e) = i.call(&cb, this.clone(), &[arr, this]) {
            let msg = super::modules::describe(i, e);
            i.console_push(alloc::format!("error: IntersectionObserver-Rueckruf: {msg}"));
        }
    }
    ran
}

/// One `MutationObserver` registration: what it wants to see on which node
/// (DOM §4.3.1 "registered observer").
pub struct MutReg {
    pub target: u32,
    /// This registration's slot in `Doc::observed`: the bit a mutation sets
    /// when it concerned this node.
    pub slot: usize,
    pub subtree: bool,
    pub child_list: bool,
    pub attrs: bool,
    pub attr_old: bool,
    pub char_data: bool,
    pub char_old: bool,
    /// `attributeFilter`: only these attributes. `None` means all.
    pub filter: Option<Vec<Rc<str>>>,
}

/// A registered `MutationObserver`.
///
/// Lives in `Interp`, not in `Doc`, because it holds a JS callback: the
/// document is rebuilt on every navigation, the realm is not.
pub struct MutObs {
    /// The JS object, i.e. the identity `observe` uses to find the entry.
    pub js: Gc,
    pub cb: Value,
    pub regs: Vec<MutReg>,
    pub queue: Vec<Mutation>,
}

/// Is anyone still observing? If not, the tree stops recording.
fn sync_observing(i: &mut Interp) {
    // Registrations get their slots, and the tree the list it needs while
    // recording.
    let mut obs: Vec<(u32, bool)> = Vec::new();
    let mut voll = false;
    for o in i.observers.iter_mut() {
        for r in o.regs.iter_mut() {
            match obs.iter().position(|(t, s)| *t == r.target && *s == r.subtree) {
                Some(k) => r.slot = k,
                None if obs.len() < 64 => { r.slot = obs.len(); obs.push((r.target, r.subtree)); }
                // More than 64 distinct observed nodes: the mask is a `u64`, and a
                // registration without a slot reports nothing. Said, not swallowed.
                None => { r.slot = usize::MAX; voll = true; }
            }
        }
    }
    if voll {
        i.console_push(alloc::string::String::from(
            "warn: MutationObserver: mehr als 64 beobachtete Knoten, die weiteren melden nichts"));
    }
    let any = !obs.is_empty();
    if let Some(d) = &mut i.doc {
        d.observing = any;
        d.observed = obs;
        if !any { d.mutations.clear(); d.mut_overflow = false; }
    }
}

/// Does this mutation match this registration?
///
/// Membership is already decided at mutation time (`Mutation::hits`);
/// here only the content filters of the registration apply.
fn matches_reg(m: &Mutation, r: &MutReg) -> bool {
    if r.slot >= 64 || m.hits & (1u64 << r.slot) == 0 { return false }
    match m.kind {
        MutKind::ChildList => r.child_list,
        MutKind::CharacterData => r.char_data,
        MutKind::Attributes => {
            if !r.attrs { return false }
            match (&r.filter, &m.attr) {
                (Some(f), Some(a)) => f.iter().any(|x| x == a),
                (Some(_), None) => false,
                (None, _) => true,
            }
        }
    }
}

/// Distribute the tree's raw record to the observers.
///
/// Filtering happens here, not while recording: the tree does not need to
/// know who observes, and one mutation can go to several observers.
pub fn collect_mutations(i: &mut Interp) {
    let raw = match &mut i.doc {
        Some(d) if !d.mutations.is_empty() => core::mem::take(&mut d.mutations),
        _ => return,
    };
    let overflow = i.doc.as_mut().map(|d| core::mem::replace(&mut d.mut_overflow, false)).unwrap_or(false);
    if overflow {
        i.console_push(alloc::string::String::from(
            "warn: MutationObserver: Aufzeichnung uebergelaufen, Meldungen fehlen"));
    }
    let mut per: Vec<Vec<Mutation>> = alloc::vec![Vec::new(); i.observers.len()];
    for m in &raw {
        for (n, o) in i.observers.iter().enumerate() {
            // An observer gets each mutation once, even if two of its registrations
            // match; otherwise observing parent and child would duplicate everything.
            if o.regs.iter().any(|r| matches_reg(m, r)) {
                // `oldValue` only if the registration asked for it; pages distinguish
                // `null` from `""`.
                let want_old = o.regs.iter().any(|r| matches_reg(m, r) && match m.kind {
                    MutKind::Attributes => r.attr_old,
                    MutKind::CharacterData => r.char_old,
                    MutKind::ChildList => false,
                });
                let mut c = m.clone();
                if !want_old { c.old = None; }
                per[n].push(c);
            }
        }
    }
    for (n, mut list) in per.into_iter().enumerate() {
        if list.is_empty() { continue }
        i.observers[n].queue.append(&mut list);
    }
}

/// Build a `MutationRecord` from a mutation.
fn build_record(i: &mut Interp, m: &Mutation) -> C<Value> {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    let kind = match m.kind {
        MutKind::ChildList => "childList",
        MutKind::Attributes => "attributes",
        MutKind::CharacterData => "characterData",
    };
    let target = wrap(i, m.target);
    let added: Vec<Value> = m.added.clone().into_iter().map(|x| wrap(i, x)).collect();
    let removed: Vec<Value> = m.removed.clone().into_iter().map(|x| wrap(i, x)).collect();
    let added = i.new_array(added);
    let removed = i.new_array(removed);
    let prev = match m.prev { Some(x) => wrap(i, x), None => Value::Null };
    let next = match m.next { Some(x) => wrap(i, x), None => Value::Null };
    {
        let mut b = o.borrow_mut();
        b.define("type", Prop::data(Value::str(kind)));
        b.define("target", Prop::data(target));
        b.define("addedNodes", Prop::data(added));
        b.define("removedNodes", Prop::data(removed));
        b.define("previousSibling", Prop::data(prev));
        b.define("nextSibling", Prop::data(next));
        b.define("attributeName", Prop::data(match &m.attr {
            Some(a) => Value::str(a), None => Value::Null }));
        // Namespaces are not tracked, so the answer is `null`.
        b.define("attributeNamespace", Prop::data(Value::Null));
        b.define("oldValue", Prop::data(match &m.old {
            Some(v) => Value::str(v), None => Value::Null }));
        b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("MutationRecord")));
    }
    Ok(Value::Obj(o))
}

/// The checkpoint: collect, and call every observer that has records.
///
/// Returns true if a callback ran; the caller must come back, because an
/// observer may change the tree in its callback.
pub fn deliver_mutations(i: &mut Interp) -> bool {
    collect_mutations(i);
    let mut ran = false;
    for n in 0..i.observers.len() {
        if i.observers[n].queue.is_empty() { continue }
        let recs = core::mem::take(&mut i.observers[n].queue);
        let cb = i.observers[n].cb.clone();
        let this = Value::Obj(i.observers[n].js.clone());
        let mut vals = Vec::with_capacity(recs.len());
        for m in &recs {
            match build_record(i, m) { Ok(v) => vals.push(v), Err(_) => return ran }
        }
        let arr = i.new_array(vals);
        ran = true;
        // A throwing callback is its own error, not the end of delivery: the
        // other observers still get their records, as in browsers.
        if let Err(e) = i.call(&cb, this.clone(), &[arr, this]) {
            let msg = super::modules::describe(i, e);
            i.console_push(alloc::format!("error: MutationObserver-Rueckruf: {msg}"));
        }
    }
    ran
}

/// The wrapper object of a node, built once and then kept.
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
        COMMENT_NODE => i.realm.comment_proto.clone(),
        _ => {
            let tag = i.doc.as_ref().map(|d| d.nodes[id as usize].tag.clone())
                       .unwrap_or_else(|| Rc::from(""));
            if &*tag == "#eventtarget" { i.realm.event_target_proto.clone() }
            else if &*tag == "#fragment" { i.realm.fragment_proto.clone() }
            else if &*tag == "svg" || tag.starts_with("svg:") {
                i.realm.tag_protos.get("svg").cloned()
                    .unwrap_or_else(|| i.realm.svg_element_proto.clone())
            } else {
                i.realm.tag_protos.get(&*tag).cloned()
                    .unwrap_or_else(|| i.realm.html_element_proto.clone())
            }
        }
    };
    let g = new_obj(Some(proto));
    g.borrow_mut().define(SLOT, Prop {
        value: Some(Value::Num(id as f64)), get: None, set: None,
        writable: false, enumerable: false, configurable: false });
    if let Some(doc) = &mut i.doc { doc.nodes[id as usize].js = Some(g.clone()); }
    Value::Obj(g)
}

/// The four positions of `insertAdjacent*` as `(parent, before)`. `None` if
/// the position is none of the four, or if `beforebegin`/`afterend` is asked
/// on a node without a parent.
fn adjacent_spot(i: &Interp, id: u32, pos: &str) -> Option<(u32, Option<u32>)> {
    let d = i.doc.as_ref()?;
    match pos {
        "afterbegin" => Some((id, d.nodes[id as usize].children.first().copied())),
        "beforeend" => Some((id, None)),
        "beforebegin" | "afterend" => {
            let p = d.nodes[id as usize].parent?;
            let kids = &d.nodes[p as usize].children;
            let k = kids.iter().position(|&c| c == id)?;
            let before = if pos == "beforebegin" { Some(id) } else { kids.get(k + 1).copied() };
            Some((p, before))
        }
        _ => None,
    }
}

/// `isEqualNode`, recursive. Attributes are compared as a set; the spec says
/// their order is irrelevant.
fn nodes_equal(d: &Doc, x: u32, y: u32) -> bool {
    if x == y {
        return true;
    }
    let (Some(a), Some(b)) = (d.nodes.get(x as usize), d.nodes.get(y as usize)) else {
        return false;
    };
    if a.kind != b.kind || a.tag != b.tag {
        return false;
    }
    if a.kind != ELEMENT_NODE && a.text != b.text {
        return false;
    }
    if a.attrs.len() != b.attrs.len() {
        return false;
    }
    for (k, v) in &a.attrs {
        if b.attr(k) != Some(v) {
            return false;
        }
    }
    a.children.len() == b.children.len()
        && a.children.iter().zip(b.children.iter()).all(|(&p, &q)| nodes_equal(d, p, q))
}

fn nodes_array(i: &mut Interp, ids: Vec<u32>) -> Value {
    let vals: Vec<Value> = ids.into_iter().map(|id| wrap(i, id)).collect();
    i.new_array(vals)
}

/// Read access to a node without holding the borrow across a call; each
/// query copies what it needs.
macro_rules! with_node {
    ($i:expr, $this:expr, |$n:ident| $body:expr) => {{
        let id = node_of($i, &$this)?;
        let Some(d) = &$i.doc else { return $i.type_err("no document") };
        let $n = &d.nodes[id as usize];
        $body
    }};
}

/// The event's slots. Non-enumerable and prefixed with `__`: a page's
/// `for (k in e)` must not see them, and `e.type` comes from the prototype,
/// not the instance.
const EV_TYPE: &str = "__evtype";
const EV_TARGET: &str = "__evtarget";
const EV_CUR: &str = "__evcur";
const EV_BUBBLES: &str = "__evbubbles";
const EV_CANCELABLE: &str = "__evcancel";
const EV_PREVENTED: &str = "__evprevented";
const EV_TRUSTED: &str = "__evtrusted";
const EV_PHASE: &str = "__evphase";
const EV_STAMP: &str = "__evstamp";
const EV_STOP: &str = "__evstop";
const EV_STOPIMM: &str = "__evstopimm";
const EV_DETAIL: &str = "__evdetail";

/// A getter reading a fixed slot. A macro because a builtin getter is a
/// function pointer that captures nothing, so the slot name must be in the
/// body, not in a variable.
macro_rules! ev_getter {
    ($proto:expr, $fp:expr, $name:literal, $slot:expr) => {{
        let g = native(Some($fp.clone()), |i, t, _| i.get(&t, $slot),
                       concat!("get ", $name), 0, false);
        $proto.borrow_mut().define($name, Prop { value: None, get: Some(Value::Obj(g)),
            set: None, writable: false, enumerable: false, configurable: true });
    }};
}

/// Build an event of a given kind as `new KeyboardEvent("keydown", {...})`
/// does: prototype from the global name, base fields from the dictionary,
/// then the fields of this kind.
///
/// A missing dictionary field gets the spec default, not `undefined`;
/// otherwise `e.clientX + 1` would be `NaN`.
fn event_of_kind(i: &mut Interp, iface: &str, a: &[Value]) -> C<Gc> {
    let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
    let init = a.get(1).cloned().unwrap_or(Value::Undefined);
    let proto = match i.get(&Value::Obj(i.realm.global.clone()), iface)
                      .and_then(|c| i.get(&c, "prototype")) {
        Ok(Value::Obj(o)) => o, _ => i.realm.event_proto.clone(),
    };
    let ev = build_event(i, proto, &kind, false);
    apply_event_init(i, &ev, &init)?;
    let has = matches!(init, Value::Obj(_));
    let set = |ev: &Gc, slot: &str, v: Value| {
        ev.borrow_mut().define(slot, Prop { value: Some(v), get: None, set: None,
            writable: true, enumerable: false, configurable: true });
    };
    // Every kind under `UIEvent` has the four modifiers and `detail`/`view`.
    for (k, slot) in [("altKey", "__evalt"), ("ctrlKey", "__evctrl"),
                      ("shiftKey", "__evshift"), ("metaKey", "__evmeta")] {
        let v = if has { i.get(&init, k)?.truthy() } else { false };
        set(&ev, slot, Value::Bool(v));
    }
    let detail = if has { i.get(&init, "detail")? } else { Value::Num(0.0) };
    set(&ev, EV_DETAIL, if matches!(detail, Value::Undefined) { Value::Num(0.0) } else { detail });
    let view = if has { i.get(&init, "view")? } else { Value::Null };
    set(&ev, "__evview", if matches!(view, Value::Undefined) { Value::Null } else { view });
    let mut num = |i: &mut Interp, k: &str, slot: &str| -> C<()> {
        let v = if has { i.get(&init, k)? } else { Value::Undefined };
        let n = match v { Value::Undefined => 0.0, other => i.to_number(&other)? };
        set(&ev, slot, Value::Num(n));
        Ok(())
    };
    match iface {
        "KeyboardEvent" => {
            for (k, slot) in [("key", "__evkey"), ("code", "__evcode")] {
                let v = if has { i.get(&init, k)? } else { Value::Undefined };
                let sv = match v { Value::Undefined => alloc::string::String::from(""),
                                   other => i.to_string(&other)?.to_string() };
                set(&ev, slot, Value::str(&sv));
            }
            num(i, "keyCode", "__evkeycode")?;
            num(i, "charCode", "__evcharcode")?;
            num(i, "location", "__evlocation")?;
            let r = if has { i.get(&init, "repeat")?.truthy() } else { false };
            set(&ev, "__evrepeat", Value::Bool(r));
            let c = if has { i.get(&init, "isComposing")?.truthy() } else { false };
            set(&ev, "__evcomposing", Value::Bool(c));
        }
        "InputEvent" => {
            let d = if has { i.get(&init, "data")? } else { Value::Null };
            set(&ev, "__evdata", if matches!(d, Value::Undefined) { Value::Null } else { d });
            let t = if has { i.get(&init, "inputType")? } else { Value::Undefined };
            let ts = match t { Value::Undefined => alloc::string::String::from(""),
                               other => i.to_string(&other)?.to_string() };
            set(&ev, "__evinputtype", Value::str(&ts));
            let c = if has { i.get(&init, "isComposing")?.truthy() } else { false };
            set(&ev, "__evcomposing", Value::Bool(c));
        }
        "FocusEvent" => {
            let r = if has { i.get(&init, "relatedTarget")? } else { Value::Null };
            set(&ev, "__evrelated", if matches!(r, Value::Undefined) { Value::Null } else { r });
        }
        "MouseEvent" => {
            for (k, slot) in [("clientX", "__evclientx"), ("clientY", "__evclienty"),
                              ("pageX", "__evpagex"), ("pageY", "__evpagey"),
                              ("offsetX", "__evoffsetx"), ("offsetY", "__evoffsety"),
                              ("button", "__evbutton"), ("buttons", "__evbuttons")] {
                num(i, k, slot)?;
            }
            let r = if has { i.get(&init, "relatedTarget")? } else { Value::Null };
            set(&ev, "__evrelated", if matches!(r, Value::Undefined) { Value::Null } else { r });
        }
        _ => {}
    }
    Ok(ev)
}

/// An event object with its slots set. `trusted` distinguishes what beak
/// itself dispatches from what the page sends with `dispatchEvent`.
fn build_event(i: &mut Interp, proto: Gc, kind: &str, trusted: bool) -> Gc {
    let ev = new_obj(Some(proto));
    let stamp = i.now_ms();
    let mut o = ev.borrow_mut();
    let hidden = |v: Value| Prop { value: Some(v), get: None, set: None,
        writable: true, enumerable: false, configurable: true };
    o.define(EV_TYPE, hidden(Value::str(kind)));
    o.define(EV_TARGET, hidden(Value::Null));
    o.define(EV_CUR, hidden(Value::Null));
    o.define(EV_BUBBLES, hidden(Value::Bool(false)));
    o.define(EV_CANCELABLE, hidden(Value::Bool(trusted)));
    o.define(EV_PREVENTED, hidden(Value::Bool(false)));
    o.define(EV_TRUSTED, hidden(Value::Bool(trusted)));
    o.define(EV_PHASE, hidden(Value::Num(0.0)));
    o.define(EV_STAMP, hidden(Value::Num(stamp)));
    o.define(EV_STOP, hidden(Value::Bool(false)));
    o.define(EV_STOPIMM, hidden(Value::Bool(false)));
    drop(o);
    ev
}

/// The fields `initEvent` sets, as a Rust function so `initCustomEvent`
/// does not write them out a second time.
///
/// DOM §initEvent explicitly resets the cancel flags: the same object may
/// be dispatched again.
fn init_event_fields(ev: &Gc, kind: &str, bubbles: bool, cancelable: bool) {
    let hidden = |v: Value| Prop { value: Some(v), get: None, set: None,
        writable: true, enumerable: false, configurable: true };
    let mut b = ev.borrow_mut();
    b.define(EV_TYPE, hidden(Value::str(kind)));
    b.define(EV_BUBBLES, hidden(Value::Bool(bubbles)));
    b.define(EV_CANCELABLE, hidden(Value::Bool(cancelable)));
    b.define(EV_PREVENTED, hidden(Value::Bool(false)));
    b.define(EV_STOP, hidden(Value::Bool(false)));
    b.define(EV_STOPIMM, hidden(Value::Bool(false)));
    b.define(EV_TARGET, hidden(Value::Null));
    b.define(EV_TRUSTED, hidden(Value::Bool(false)));
}

/// `new Event(type, {bubbles, cancelable})`: the second argument.
fn apply_event_init(i: &mut Interp, ev: &Gc, init: &Value) -> C<()> {
    if !matches!(init, Value::Obj(_)) { return Ok(()) }
    for (key, slot) in [("bubbles", EV_BUBBLES), ("cancelable", EV_CANCELABLE),
                        ("composed", "__evcomposed")] {
        let v = i.get(init, key)?;
        let b = v.truthy();
        ev.borrow_mut().define(slot, Prop { value: Some(Value::Bool(b)), get: None, set: None,
            writable: true, enumerable: false, configurable: true });
    }
    Ok(())
}

/// Are these the same function? Identity, not equality; that is what
/// `removeEventListener` asks.
fn same_fn(a: &Value, b: &Value) -> bool {
    match (a, b) { (Value::Obj(x), Value::Obj(y)) => Rc::ptr_eq(x, y), _ => false }
}

/// The propagation path for a node: from the root to it.
///
/// Same order in which beak builds it from layout: outermost first, target
/// last. Reversing it reverses bubbling.
fn ancestors(i: &Interp, id: u32) -> Vec<u32> {
    let Some(d) = &i.doc else { return alloc::vec![id] };
    let mut out = alloc::vec![id];
    let mut cur = d.nodes[id as usize].parent;
    while let Some(x) = cur {
        out.push(x);
        cur = d.nodes[x as usize].parent;
    }
    out.reverse();
    out
}

/// The declarations of a `style` attribute, in source order.
///
/// A small parser of its own, not the one from `css`: this one must return
/// exactly what a script wrote, while the cascade parser drops invalid
/// declarations, and `el.style.foo` would then read something else.
fn style_decls(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in text.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || v.is_empty() { continue }
        out.push((k.to_ascii_lowercase(), v.to_string()));
    }
    out
}

fn style_join(decls: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in decls {
        if !out.is_empty() { out.push(' '); }
        out.push_str(k); out.push_str(": "); out.push_str(v); out.push(';');
    }
    out
}

/// The snapshot `getComputedStyle` stores. If present, the declaration
/// reads it instead of the `style` attribute: the same accessors, two
/// sources.
const COMPUTED: &str = "__computed";

/// The declaration text behind a `CSSStyleDeclaration`.
fn style_text(i: &Interp, this: &Value) -> String {
    if let Value::Obj(o) = this {
        if let Some(Value::Str(t)) = o.borrow().get_own(COMPUTED).and_then(|p| p.value.clone()) {
            return t.to_string();
        }
    }
    let Ok(id) = node_of_ref(i, this) else { return String::new() };
    i.doc.as_ref()
        .and_then(|d| d.nodes[id as usize].attr("style").map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Like `node_of`, but without throwing; a snapshot has no node.
fn node_of_ref(_i: &Interp, v: &Value) -> Result<u32, ()> {
    let Value::Obj(o) = v else { return Err(()) };
    match o.borrow().get_own(SLOT).and_then(|p| p.value.clone()) {
        Some(Value::Num(n)) if n >= 0.0 => Ok(n as u32),
        _ => Err(()),
    }
}

fn style_get(i: &Interp, id: u32, css: &str) -> Value {
    let Some(d) = &i.doc else { return Value::str("") };
    let text = d.nodes[id as usize].attr("style").map(|s| s.to_string()).unwrap_or_default();
    match style_decls(&text).into_iter().rev().find(|(k, _)| k == css) {
        Some((_, v)) => Value::string(v),
        // Unset is the empty string, not `undefined`, as the spec says; pages
        // check for it.
        None => Value::str(""),
    }
}

/// Set a declaration, or remove it when the value is empty.
///
/// The result goes into the `style` attribute, not a side store: the
/// cascade reads the attribute, so `el.style.display = "none"` takes
/// effect.
fn style_set(i: &mut Interp, id: u32, css: &str, val: &str) {
    let Some(d) = &mut i.doc else { return };
    let text = d.nodes[id as usize].attr("style").map(|s| s.to_string()).unwrap_or_default();
    let mut decls = style_decls(&text);
    decls.retain(|(k, _)| k != css);
    let v = val.trim();
    if !v.is_empty() { decls.push((css.to_string(), v.to_string())); }
    let joined = style_join(&decls);
    d.set_attr_at(id, "style", &joined);
    d.touch();
}

/// A property pair on `CSSStyleDeclaration.prototype`. A macro for the same
/// reason as `ev_getter`: a builtin getter is a function pointer and
/// captures nothing.
macro_rules! style_prop {
    ($proto:expr, $fp:expr, $js:literal, $css:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            let text = style_text(i, &t);
            Ok(match style_decls(&text).into_iter().rev().find(|(k, _)| k == $css) {
                Some((_, v)) => Value::string(v),
                None => Value::str(""),
            })
        }, concat!("get ", $js), 0, false);
        let st = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            style_set(i, id, $css, &v);
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(st)), writable: false, enumerable: false, configurable: true });
    }};
}

/// A handler as a property: `el.onclick`. A macro because the getter is a
/// function pointer and the event type must be in the body.
macro_rules! handler_prop {
    ($proto:expr, $fp:expr, $js:literal, $kind:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            let id = target_node(i, &t)?;
            if let Some(f) = i.doc.as_ref().and_then(|d| d.nodes[id as usize].handlers.iter()
                .find(|(k, _)| &**k == $kind).map(|(_, f)| f.clone())) { return Ok(f) }
            // With only the attribute present, browsers still return a function, so
            // it is compiled here, as on dispatch.
            Ok(inline_handler(i, id, $kind)?.unwrap_or(Value::Null))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = target_node(i, &t)?;
            let f = a.first().cloned().unwrap_or(Value::Null);
            let callable = i.is_callable(&f);
            if let Some(d) = &mut i.doc {
                d.nodes[id as usize].handlers.retain(|(k, _)| &**k != $kind);
                if callable {
                    d.nodes[id as usize].handlers.push((Rc::from($kind), f));
                    // Without this, layout records no hit boxes and the click finds
                    // nothing; the same trap as with `addEventListener`.
                    d.has_listeners = true;
                }
            }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// A field backed by an attribute: `a.href`, `img.src`, `el.title`.
/// Reads the empty string if the attribute is missing, not `undefined`,
/// because page code calls `.indexOf` on it.
macro_rules! attr_prop {
    ($proto:expr, $fp:expr, $js:literal, $attr:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr($attr) {
                Some(v) => Value::Str(v.clone()), None => Value::str("") }))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc { d.set_attr_at(id, $attr, &v); }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// Like `attr_prop!`, but `null` instead of the empty string when the
/// attribute is missing. That is how ARIA reflection works (ARIA 1.2 §9):
/// the fields are `DOMString?`.
macro_rules! attr_prop_null {
    ($proto:expr, $fp:expr, $js:literal, $attr:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr($attr) {
                Some(v) => Value::Str(v.clone()), None => Value::Null }))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            match a.first() {
                // `el.ariaHidden = null` removes the attribute. Setting it to the string
                // "null" would be wrong: `aria-hidden="null"` is true, since every value
                // except "false" is.
                None | Some(Value::Null) | Some(Value::Undefined) => {
                    if let Some(d) = &mut i.doc { d.remove_attr_at(id, $attr); }
                }
                Some(v) => {
                    let v = v.clone();
                    let v = i.to_string(&v)?;
                    if let Some(d) = &mut i.doc { d.set_attr_at(id, $attr, &v); }
                }
            }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// A field that is a number on an attribute: `el.tabIndex`. If the
/// attribute is missing or not a number, `default` applies; for `tabIndex`
/// that is -1 for anything not focusable by default.
macro_rules! num_attr_prop {
    ($proto:expr, $fp:expr, $js:literal, $attr:literal, $default:expr) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            with_node!(i, t, |n| Ok(Value::Num(match n.attr($attr) {
                Some(v) => v.trim().parse::<f64>().unwrap_or($default),
                None => $default })))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
            let v = i.to_string(&Value::Num(n))?;
            if let Some(d) = &mut i.doc { d.set_attr_at(id, $attr, &v); }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// A field that is the presence of an attribute: `el.hidden`,
/// `script.async`. The value does not matter: `hidden="false"` still hides.
macro_rules! bool_attr_prop {
    ($proto:expr, $fp:expr, $js:literal, $attr:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            with_node!(i, t, |n| Ok(Value::Bool(n.attr($attr).is_some())))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            let on = a.first().map(|v| v.truthy()).unwrap_or(false);
            if let Some(d) = &mut i.doc {
                d.touch();
                if on { d.set_attr_at(id, $attr, "") }
                else { d.remove_attr_at(id, $attr) }
            }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// The nodes of a parsed fragment: the `<html>`/`<head>`/`<body>` frame the
/// document parser always builds is dropped.
fn fragment_nodes(root: &crate::dom::Element) -> Vec<&crate::dom::Node> {
    let mut out = Vec::new();
    for c in &root.children {
        match c {
            crate::dom::Node::Element(e) if &*e.tag == "html" => {
                for c2 in &e.children {
                    match c2 {
                        crate::dom::Node::Element(e2) if &*e2.tag == "head" || &*e2.tag == "body" =>
                            out.extend(e2.children.iter()),
                        other => out.push(other),
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The number under which layout knows this element.
///
/// `to_dom` assigns fresh `seq` on write-back; before that they are 0 and
/// layout still comes from the parsed tree, whose numbers are in
/// `src_seq`. "Non-zero wins" therefore means "has anyone written back
/// yet?".
fn layout_seq(i: &Interp, this: &Value) -> Option<u32> {
    let id = node_of_ref(i, this).ok()?;
    let n = &i.doc.as_ref()?.nodes[id as usize];
    Some(if n.seq != 0 { n.seq } else { n.src_seq }).filter(|s| *s != 0)
}

/// Cap against layout thrashing: how often layout may be forced between two
/// host frames. A page that writes and reads in a loop would otherwise
/// force a full layout per iteration.
const FORCED_LAYOUT_CAP: u32 = 4;

/// Before any box query: does this element have a box yet?
///
/// If not, and the tree changed since the last frame and the element is
/// connected, the zero is a missing frame, not an answer, so layout runs
/// now.
///
/// Deliberately narrow: browsers recompute on every read of a dirty tree;
/// here layout only runs when there is no box at all, so read loops over
/// existing elements cost nothing. Not handled: an element that moved since
/// the last frame still reports its old position.
fn ensure_box(i: &mut Interp, this: &Value) {
    if i.relayout.is_none() || i.in_forced_layout { return }
    // Already has a box: nothing to do. The common case, and it must stay
    // cheap.
    if let Some(seq) = layout_seq(i, this) {
        if i.geometry.as_ref().is_some_and(|g| g.boxes.iter().any(|b| b.seq == seq)) { return }
    }
    // Only if the tree changed. On a clean tree the zero is the truth
    // (`display:none`, an empty inline) and layout would not change it.
    if !i.doc.as_ref().is_some_and(|d| d.dirty) { return }
    // And only for a connected node. A detached one gets no box from layout
    // either, and every read would relayout.
    let Ok(id) = node_of_ref(i, this) else { return };
    if !is_in_document(i, id) { return }
    force_layout(i);
}

fn is_in_document(i: &Interp, id: u32) -> bool {
    let Some(d) = &i.doc else { return false };
    let mut cur = Some(id);
    while let Some(x) = cur {
        if x == d.doc { return true }
        cur = d.nodes[x as usize].parent;
    }
    false
}

/// Call the host hook: once, under the cap, with a console line when the
/// cap hits. A cap that silently returns a wrong number is worse than none.
fn force_layout(i: &mut Interp) {
    if i.forced_layouts >= FORCED_LAYOUT_CAP {
        if i.forced_layouts == FORCED_LAYOUT_CAP {
            i.forced_layouts += 1;
            i.console_push(alloc::string::String::from(
                "warn: mehr als 4 erzwungene Auslegungen in einem Bild — ab hier antworten Kastenfragen aus dem letzten Bild"));
        }
        return
    }
    let Some(f) = i.relayout else { return };
    i.in_forced_layout = true;
    f(i);
    i.in_forced_layout = false;
    i.forced_layouts += 1;
}

/// The border box in viewport coordinates: `(x, y, w, h)`.
///
/// A box may break into several fragments (an inline box per line);
/// `getBoundingClientRect` returns their union, as browsers do.
fn elem_rect(i: &Interp, this: &Value) -> Option<(f64, f64, f64, f64)> {
    let g = i.geometry.as_ref()?;
    let seq = layout_seq(i, this)?;
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for b in g.boxes.iter().filter(|b| b.seq == seq) {
        acc = Some(match acc {
            None => (b.x, b.y, b.x + b.w, b.y + b.h),
            Some((x0, y0, x1, y1)) =>
                (x0.min(b.x), y0.min(b.y), x1.max(b.x + b.w), y1.max(b.y + b.h)),
        });
    }
    let (x0, y0, x1, y1) = acc?;
    Some(((x0 - g.scroll.0) as f64, (y0 - g.scroll.1) as f64, (x1 - x0) as f64, (y1 - y0) as f64))
}

/// The padding box: width and height without the borders.
fn elem_inner(i: &Interp, this: &Value) -> Option<(f64, f64)> {
    let g = i.geometry.as_ref()?;
    let seq = layout_seq(i, this)?;
    let (_, _, w, h) = elem_rect(i, this)?;
    // The borders of the first fragment: a box broken across lines draws them
    // only at its outer ends, where the value is 0 anyway.
    let b = g.boxes.iter().find(|b| b.seq == seq)?;
    Some(((w - b.bx as f64).max(0.0), (h - b.by as f64).max(0.0)))
}

/// A `DOMRect`-like object. `None` means no box and yields all zeros, the
/// same answer browsers give for an element without a box (`display:none`).
fn rect_obj(i: &Interp, r: Option<(f64, f64, f64, f64)>) -> Gc {
    let (x, y, w, h) = r.unwrap_or((0.0, 0.0, 0.0, 0.0));
    let o = new_obj(Some(i.realm.object_proto.clone()));
    for (k, v) in [("x", x), ("y", y), ("left", x), ("top", y),
                   ("right", x + w), ("bottom", y + h), ("width", w), ("height", h)] {
        o.borrow_mut().define(k, Prop::data(Value::Num(v)));
    }
    o
}

/// Where `append` and friends insert.
enum Where { First, Last, Before, After }

/// The shared body of `append`/`prepend`/`before`/`after`. An argument that
/// is not a node becomes a text node; that distinguishes this family from
/// `appendChild`.
fn insert_all(i: &mut Interp, this: &Value, args: &[Value], w: Where) -> C<Value> {
    let me = node_of(i, this)?;
    let (parent, anchor) = match w {
        Where::First => (me, i.doc.as_ref().and_then(|d| d.nodes[me as usize].children.first().copied())),
        Where::Last => (me, None),
        Where::Before | Where::After => {
            let Some(p) = i.doc.as_ref().and_then(|d| d.nodes[me as usize].parent) else {
                return Ok(Value::Undefined)
            };
            let after = matches!(w, Where::After);
            let sib = i.doc.as_ref().and_then(|d| {
                let ks = &d.nodes[p as usize].children;
                let k = ks.iter().position(|&c| c == me)?;
                if after { ks.get(k + 1).copied() } else { Some(me) }
            });
            (p, sib)
        }
    };
    for v in args {
        let id = match node_of(i, v) {
            Ok(x) => x,
            Err(_) => {
                let s = i.to_string(v)?;
                let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
                let t = d.create(TEXT_NODE, "");
                d.nodes[t as usize].text = s;
                t
            }
        };
        // The anchor stays the same: everything goes before it, so multiple
        // arguments end up in the order they were passed.
        if let Some(d) = &mut i.doc { d.insert_maybe_fragment(parent, id, anchor); d.touch(); }
        fire_connected(i, id)?;
    }
    Ok(Value::Undefined)
}

/// The tree the cascade runs on: the live one, built from `doc` and rebuilt
/// only when `doc.version` has moved. A script that sets a class and then
/// measures must see the change; the cache means one build per batch of
/// changes, not per query.
fn style_tree(i: &Interp) -> Option<alloc::rc::Rc<crate::dom::Dom>> {
    let doc = i.doc.as_ref()?;
    let mut slot = i.live_dom.borrow_mut();
    if !matches!(&*slot, Some((v, _)) if *v == doc.version) {
        *slot = Some((doc.version, alloc::rc::Rc::new(doc.live_dom())));
    }
    slot.as_ref().map(|(_, d)| d.clone())
}

/// The cascaded style of an element as declaration text.
///
/// `node` is the element's arena index, the same number `live_dom` writes
/// as `seq` into the tree. The path from the root is resolved level by
/// level on every call. `None` if no style context was provided or the
/// element is not in the tree; the caller then falls back to the inline
/// style.
fn computed_decls(i: &Interp, node: u32) -> Option<String> {
    let ctx = i.style_ctx.as_ref()?;
    let tree = style_tree(i)?;
    let mut path: Vec<&crate::dom::Element> = Vec::new();
    if !find_path(&tree.root, node, &mut path) {
        return None;
    }
    let mut parent = crate::style::ComputedStyle::root(&ctx.theme);
    parent.vw = ctx.viewport_w;
    let mut anc: Vec<crate::css::ElemInfo> = Vec::new();
    // The variable map goes along; without it `:root`'s custom properties
    // never reach the element and `getComputedStyle` would disagree with
    // what layout painted.
    let mut vars = crate::vars::VarMap::new();
    let mut out = parent;
    for (k, el) in path.iter().enumerate() {
        // Count siblings so `:nth-*` and `:first-child` match the same way as in
        // layout.
        let (prev, count) = match k {
            0 => (Vec::new(), 1),
            _ => {
                let kids: Vec<&crate::dom::Element> = path[k - 1]
                    .children.iter()
                    .filter_map(|n| match n { crate::dom::Node::Element(e) => Some(e), _ => None })
                    .collect();
                let pos = kids.iter().position(|c| c.seq == el.seq).unwrap_or(0);
                (kids[..pos].iter().map(|e| crate::css::ElemInfo {
                    el: e, state: Default::default() }).collect(), kids.len() as u32)
            }
        };
        let info = crate::css::ElemInfo { el, state: Default::default() };
        let mut own = None;
        out = crate::style::resolve_in(&info, &parent, &ctx.theme, &ctx.sheet,
                                       &anc, &prev, count, ctx.viewport_w, &vars, &mut own);
        // A flex or grid item is blockified (css-display-3 §2.7), and
        // `layout_flex` computes with exactly that.
        if matches!(parent.display, crate::style::Display::Flex
                    | crate::style::Display::InlineFlex | crate::style::Display::Grid) {
            out.display = crate::style::blockify(out.display);
        }
        if let Some(m) = own { vars = m; }
        // `rem` resolves against the root, which is only known once it is
        // resolved. Layout sets it right after the root pass; this mirrors it.
        if k == 0 {
            out.rem_base = out.font_px;
        }
        parent = out;
        anc.push(info);
    }
    Some(crate::style::serialize_computed(&out))
}

/// Collect the path from the root to `seq`.
fn find_path<'a>(el: &'a crate::dom::Element, seq: u32,
                 out: &mut Vec<&'a crate::dom::Element>) -> bool {
    if el.seq == seq {
        out.push(el);
        return true;
    }
    for c in &el.children {
        if let crate::dom::Node::Element(e) = c {
            if find_path(e, seq, out) {
                out.insert(0, el);
                return true;
            }
        }
    }
    false
}

/// A function on the window. `meth` puts it on a prototype; this one
/// belongs on the global object itself.
fn def_global(realm: &Realm, name: &str, f: NativeFn, len: usize, fp: &Gc) {
    let g = native(Some(fp.clone()), f, name, len, false);
    realm.global.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
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


// ── XPath ───────────────────────────────────────────────────────────────────
//
// The language pages actually write (htmx finds its `hx-on:` attributes
// with it); `js/xpath.rs` lists what is missing.

/// Fetch the expression: parsed once, cached under its source text.
fn xpath_compiled(i: &mut Interp, src: &str) -> C<Rc<super::xpath::XPath>> {
    if let Some((s, x)) = &i.xpath_memo {
        if s == src {
            return Ok(x.clone());
        }
    }
    match super::xpath::parse(src) {
        Ok(x) => {
            let x = Rc::new(x);
            i.xpath_memo = Some((src.to_string(), x.clone()));
            Ok(x)
        }
        // An unparsable expression is an error, not an empty result: an empty set
        // looks like "nothing found".
        Err(e) => Err(i.throw_kind("SyntaxError", &alloc::format!("XPath: {e}"))),
    }
}

/// An attribute node as an `Attr` object, the same shape
/// `element.attributes` returns.
fn wrap_attr(i: &mut Interp, owner: u32, k: usize) -> Value {
    let pair = i.doc.as_ref()
        .and_then(|d| d.nodes.get(owner as usize))
        .and_then(|n| n.attrs.get(k))
        .cloned();
    let Some((name, val)) = pair else { return Value::Null };
    let a = new_obj(Some(i.realm.attr_proto.clone()));
    let hide = |v: Value| Prop { value: Some(v), get: None, set: None,
        writable: false, enumerable: false, configurable: false };
    let mut b = a.borrow_mut();
    b.define("__attrname", hide(Value::Str(name)));
    b.define("__attrval", hide(Value::Str(val)));
    drop(b);
    Value::Obj(a)
}

fn wrap_xnode(i: &mut Interp, n: super::xpath::XNode) -> Value {
    match n {
        super::xpath::XNode::Node(id) => wrap(i, id),
        super::xpath::XNode::Attr(o, k) => wrap_attr(i, o, k),
    }
}

/// `document.evaluate` / `XPathExpression.evaluate` both end here.
fn xpath_run(i: &mut Interp, src: &str, ctx: &Value, want: f64) -> C<Value> {
    let id = node_of(i, ctx)?;
    let expr = xpath_compiled(i, src)?;
    let Some(doc) = i.doc.as_ref() else { return i.type_err("XPath: no document") };
    let val = expr.eval(doc, super::xpath::XNode::Node(id));
    // Evaluate first, then wrap: evaluation borrows `i.doc`, wrapping needs
    // `i` mutably.
    let (nodes, num, string, boolean) = {
        let doc = i.doc.as_ref().expect("checked");
        let num = super::xpath::to_number(doc, &val);
        let string = super::xpath::to_str(doc, &val);
        let boolean = super::xpath::to_boolean(doc, &val);
        (super::xpath::result_nodes(doc, val), num, string, boolean)
    };
    let r = new_obj(Some(i.realm.xpath_result_proto.clone()));
    let hide = |v: Value| Prop { value: Some(v), get: None, set: None,
        writable: false, enumerable: false, configurable: false };
    // `ANY_TYPE` (0) reports the natural type of the result; otherwise the
    // requested type applies.
    let ty = if want == 0.0 {
        if !nodes.is_empty() || string.is_empty() && num.is_nan() { 4.0 } else { 4.0 }
    } else {
        want
    };
    let vals: Vec<Value> = nodes.into_iter().map(|n| wrap_xnode(i, n)).collect();
    let arr = i.new_array(vals);
    {
        let mut b = r.borrow_mut();
        b.define("__xnodes", hide(arr));
        b.define("__xi", Prop { value: Some(Value::Num(0.0)), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        b.define("__xtype", hide(Value::Num(ty)));
        b.define("__xnum", hide(Value::Num(num)));
        b.define("__xstr", hide(Value::string(string)));
        b.define("__xbool", hide(Value::Bool(boolean)));
    }
    Ok(Value::Obj(r))
}

fn xpath_nodes(i: &mut Interp, t: &Value) -> C<Vec<Value>> {
    let arr = i.get(t, "__xnodes")?;
    let len = i.get(&arr, "length")?;
    let len = i.to_number(&len)? as usize;
    let mut out = Vec::with_capacity(len);
    for k in 0..len {
        out.push(i.get(&arr, &alloc::format!("{k}"))?);
    }
    Ok(out)
}

/// `FormData`: a form's entry list for `fetch`.
///
/// The pairs are stored as a field on the object, in document order, with
/// the same rules for successful controls as `forms::submit`: named, not
/// disabled, and a checkbox only when checked.
///
/// Not implemented: files. beak has no `multipart/form-data`, and an empty
/// `File` would be worse than none.
fn install_formdata(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.object_proto.clone()));
    proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("FormData")));

    let ctor = native(Some(fp.clone()), |i, _, a| {
        let o = new_obj(Some(i.realm.formdata_proto.clone()));
        let mut pairs: Vec<Value> = Vec::new();
        if let Some(v @ Value::Obj(_)) = a.first() {
            if let Ok(form) = node_of(i, v) {
                for c in form_controls(i, form) {
                    let (name, ok, textarea) = {
                        let d = i.doc.as_ref();
                        let n = d.and_then(|d| d.nodes.get(c as usize));
                        let name = n.and_then(|n| n.attr("name")).map(|s| s.to_string()).unwrap_or_default();
                        let dis = n.is_some_and(|n| n.attr("disabled").is_some());
                        let ty = n.and_then(|n| n.attr("type").map(|t| t.to_ascii_lowercase())).unwrap_or_default();
                        let tag = n.map(|n| n.tag.to_string()).unwrap_or_default();
                        let boxlike = tag == "input" && matches!(ty.as_str(), "checkbox" | "radio");
                        let on = !boxlike || checked_now(i, c);
                        // A button is only successful if it is the submitter; here there is
                        // none.
                        let button = tag == "button" || matches!(ty.as_str(), "submit" | "reset" | "button" | "image");
                        (name.clone(), !name.is_empty() && !dis && on && !button, tag == "textarea")
                    };
                    if !ok { continue }
                    let val = control_value(i, c, textarea);
                    let pair = i.new_array(alloc::vec![Value::string(name), Value::string(val)]);
                    pairs.push(pair);
                }
            }
        }
        let arr = i.new_array(pairs);
        o.borrow_mut().define("__pairs", Prop { value: Some(arr), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        Ok(Value::Obj(o))
    }, "FormData", 0, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    realm.global.borrow_mut().define("FormData", Prop::builtin(Value::Obj(ctor)));
    realm.formdata_proto = proto.clone();

    meth(&proto, "append", |i, t, a| {
        let (k, v) = (i.to_string(a.first().unwrap_or(&Value::Undefined))?,
                      i.to_string(a.get(1).unwrap_or(&Value::Undefined))?);
        let pair = i.new_array(alloc::vec![Value::Str(k), Value::Str(v)]);
        let arr = i.get(&t, "__pairs")?;
        let push = i.get(&arr, "push")?;
        i.call(&push, arr, &[pair])?;
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&proto, "get", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(fd_all(i, &t, &k)?.into_iter().next().unwrap_or(Value::Null))
    }, 1, &fp);
    meth(&proto, "getAll", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let v = fd_all(i, &t, &k)?;
        Ok(i.new_array(v))
    }, 1, &fp);
    meth(&proto, "has", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(!fd_all(i, &t, &k)?.is_empty()))
    }, 1, &fp);
    meth(&proto, "set", |i, t, a| {
        let (k, v) = (i.to_string(a.first().unwrap_or(&Value::Undefined))?,
                      i.to_string(a.get(1).unwrap_or(&Value::Undefined))?);
        let kept = fd_pairs(i, &t)?.into_iter()
            .filter(|(pk, _)| *pk != *k)
            .map(|(pk, pv)| i.new_array(alloc::vec![Value::string(pk), Value::string(pv)]))
            .collect::<Vec<_>>();
        let mut kept = kept;
        kept.push(i.new_array(alloc::vec![Value::Str(k), Value::Str(v)]));
        let arr = i.new_array(kept);
        i.set(&t, "__pairs", arr, false)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&proto, "delete", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let kept = fd_pairs(i, &t)?.into_iter()
            .filter(|(pk, _)| *pk != *k)
            .map(|(pk, pv)| i.new_array(alloc::vec![Value::string(pk), Value::string(pv)]))
            .collect::<Vec<_>>();
        let arr = i.new_array(kept);
        i.set(&t, "__pairs", arr, false)?;
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&proto, "forEach", |i, t, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("FormData.forEach needs a function") }
        for (k, v) in fd_pairs(i, &t)? {
            i.call(&f, Value::Undefined, &[Value::string(v), Value::string(k), t.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    for name in ["entries", "keys", "values"] {
        let which = name;
        let f: NativeFn = match which {
            "keys" => |i, t, _| {
                let v: Vec<Value> = fd_pairs(i, &t)?.into_iter().map(|(k, _)| Value::string(k)).collect();
                let a = i.new_array(v); let it = i.get(&a, SYM_ITERATOR)?; i.call(&it, a, &[])
            },
            "values" => |i, t, _| {
                let v: Vec<Value> = fd_pairs(i, &t)?.into_iter().map(|(_, v)| Value::string(v)).collect();
                let a = i.new_array(v); let it = i.get(&a, SYM_ITERATOR)?; i.call(&it, a, &[])
            },
            _ => |i, t, _| {
                let v: Vec<Value> = fd_pairs(i, &t)?.into_iter()
                    .map(|(k, val)| i.new_array(alloc::vec![Value::string(k), Value::string(val)])).collect();
                let a = i.new_array(v); let it = i.get(&a, SYM_ITERATOR)?; i.call(&it, a, &[])
            },
        };
        meth(&proto, name, f, 0, &fp);
    }
}

/// The pairs of a `FormData` as Rust values.
fn fd_pairs(i: &mut Interp, t: &Value) -> C<Vec<(alloc::string::String, alloc::string::String)>> {
    let arr = i.get(t, "__pairs")?;
    let len = i.get(&arr, "length")?;
    let len = i.to_number(&len)? as usize;
    let mut out = Vec::with_capacity(len);
    for k in 0..len {
        let pair = i.get(&arr, &alloc::format!("{k}"))?;
        let a = i.get(&pair, "0")?;
        let b = i.get(&pair, "1")?;
        out.push((i.to_string(&a)?.to_string(), i.to_string(&b)?.to_string()));
    }
    Ok(out)
}

fn fd_all(i: &mut Interp, t: &Value, key: &str) -> C<Vec<Value>> {
    Ok(fd_pairs(i, t)?.into_iter().filter(|(k, _)| k == key).map(|(_, v)| Value::string(v)).collect())
}

fn install_xpath(realm: &mut Realm) {
    let fp = realm.function_proto.clone();

    // ── XPathResult ──────────────────────────────────────────────────────
    let res_proto = new_obj(Some(realm.object_proto.clone()));
    let res_ctor = native(Some(fp.clone()),
        |i, _, _| i.type_err("Illegal constructor"), "XPathResult", 0, true);
    res_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(res_proto.clone())));
    res_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(res_ctor.clone())));
    res_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("XPathResult")));
    // The ten type constants are on both the constructor
    // (`XPathResult.FIRST_ORDERED_NODE_TYPE`) and the instance.
    for (name, v) in [
        ("ANY_TYPE", 0.0), ("NUMBER_TYPE", 1.0), ("STRING_TYPE", 2.0),
        ("BOOLEAN_TYPE", 3.0), ("UNORDERED_NODE_ITERATOR_TYPE", 4.0),
        ("ORDERED_NODE_ITERATOR_TYPE", 5.0), ("UNORDERED_NODE_SNAPSHOT_TYPE", 6.0),
        ("ORDERED_NODE_SNAPSHOT_TYPE", 7.0), ("ANY_UNORDERED_NODE_TYPE", 8.0),
        ("FIRST_ORDERED_NODE_TYPE", 9.0),
    ] {
        res_ctor.borrow_mut().define(name, Prop::frozen(Value::Num(v)));
        res_proto.borrow_mut().define(name, Prop::frozen(Value::Num(v)));
    }
    realm.global.borrow_mut().define("XPathResult", Prop::builtin(Value::Obj(res_ctor)));

    meth(&res_proto, "iterateNext", |i, t, _| {
        let at = i.get(&t, "__xi")?;
        let at = i.to_number(&at)? as usize;
        let nodes = xpath_nodes(i, &t)?;
        match nodes.get(at) {
            Some(v) => {
                i.set(&t, "__xi", Value::Num((at + 1) as f64), false)?;
                Ok(v.clone())
            }
            // Exhausted: `null`, which ends the caller's `while` loop.
            None => Ok(Value::Null),
        }
    }, 0, &fp);

    meth(&res_proto, "snapshotItem", |i, t, a| {
        let k = i.to_number(a.first().unwrap_or(&Value::Num(0.0)))? as usize;
        Ok(xpath_nodes(i, &t)?.get(k).cloned().unwrap_or(Value::Null))
    }, 1, &fp);

    getter(&res_proto, "resultType", |i, t, _| i.get(&t, "__xtype"), &fp);
    getter(&res_proto, "numberValue", |i, t, _| i.get(&t, "__xnum"), &fp);
    getter(&res_proto, "stringValue", |i, t, _| i.get(&t, "__xstr"), &fp);
    getter(&res_proto, "booleanValue", |i, t, _| i.get(&t, "__xbool"), &fp);
    getter(&res_proto, "snapshotLength", |i, t, _| {
        Ok(Value::Num(xpath_nodes(i, &t)?.len() as f64))
    }, &fp);
    getter(&res_proto, "singleNodeValue", |i, t, _| {
        Ok(xpath_nodes(i, &t)?.first().cloned().unwrap_or(Value::Null))
    }, &fp);
    realm.xpath_result_proto = res_proto;

    // ── XPathExpression ──────────────────────────────────────────────────
    let expr_proto = new_obj(Some(realm.object_proto.clone()));
    let expr_ctor = native(Some(fp.clone()),
        |i, _, _| i.type_err("Illegal constructor"), "XPathExpression", 0, true);
    expr_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(expr_proto.clone())));
    expr_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(expr_ctor.clone())));
    expr_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("XPathExpression")));
    realm.global.borrow_mut().define("XPathExpression", Prop::builtin(Value::Obj(expr_ctor)));
    meth(&expr_proto, "evaluate", |i, t, a| {
        let src = i.get(&t, "__xsrc")?;
        let src = i.to_string(&src)?.to_string();
        let ctx = a.first().cloned().unwrap_or(Value::Undefined);
        let want = match a.get(1) { Some(v) => i.to_number(v)?, None => 0.0 };
        xpath_run(i, &src, &ctx, want)
    }, 1, &fp);
    realm.xpath_expr_proto = expr_proto;

    // ── XPathEvaluator ───────────────────────────────────────────────────
    let ev_proto = new_obj(Some(realm.object_proto.clone()));
    let ev_ctor = native(Some(fp.clone()), |i, _, _| {
        Ok(Value::Obj(new_obj(Some(i.realm.xpath_eval_proto.clone()))))
    }, "XPathEvaluator", 0, true);
    ev_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ev_proto.clone())));
    ev_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ev_ctor.clone())));
    ev_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("XPathEvaluator")));
    realm.global.borrow_mut().define("XPathEvaluator", Prop::builtin(Value::Obj(ev_ctor)));
    meth(&ev_proto, "createExpression", |i, _, a| {
        let src = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        // Parse now, not at evaluation: `createExpression` is where browsers
        // report a syntax error.
        xpath_compiled(i, &src)?;
        let o = new_obj(Some(i.realm.xpath_expr_proto.clone()));
        o.borrow_mut().define("__xsrc", Prop { value: Some(Value::string(src)),
            get: None, set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(o))
    }, 1, &fp);
    meth(&ev_proto, "evaluate", |i, _, a| {
        let src = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let ctx = a.get(1).cloned().unwrap_or(Value::Undefined);
        let want = match a.get(3) { Some(v) => i.to_number(v)?, None => 0.0 };
        xpath_run(i, &src, &ctx, want)
    }, 4, &fp);
    // `createNSResolver` exists so the usual call sequence does not throw; it
    // does not resolve namespaces (see `js/xpath.rs`).
    meth(&ev_proto, "createNSResolver", |_, _, a| {
        Ok(a.first().cloned().unwrap_or(Value::Null))
    }, 1, &fp);
    realm.xpath_eval_proto = ev_proto;
}

/// Builds the `Node`/`Element`/`Document` prototypes and the global
/// `document`.
pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    // `EventTarget` sits below `Node`: `addEventListener` belongs there, not
    // on the node, otherwise `window` would not have it.
    let event_target_proto = new_obj(Some(realm.object_proto.clone()));
    let node_proto = new_obj(Some(event_target_proto.clone()));
    let element_proto = new_obj(Some(node_proto.clone()));
    let text_proto = new_obj(Some(node_proto.clone()));
    let document_proto = new_obj(Some(node_proto.clone()));
    // `DocumentFragment` hangs off Node, not Element; set up here because the
    // query functions further down also live on it.
    let fragment_proto = new_obj(Some(node_proto.clone()));

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
            // All children out, one text node in. The old nodes stay in the arena,
            // just detached: freeing them would shift indices, and a handle must
            // never move.
            d.clear_children(id);
            if !s.is_empty() {
                let tid = d.create(TEXT_NODE, "#text");
                d.nodes[tid as usize].text = s;
                d.append(id, tid);
            }
            Ok(Value::Undefined)
        }, &fp);
    // `normalize`: merge adjacent text nodes, remove empty ones.
    meth(&node_proto, "normalize", |i, t, _| {
        let id = node_of(i, &t)?;
        fn walk(d: &mut Doc, id: u32) {
            let kids = d.nodes[id as usize].children.clone();
            let mut keep: Vec<u32> = Vec::with_capacity(kids.len());
            for c in kids {
                if d.nodes[c as usize].kind == TEXT_NODE {
                    if d.nodes[c as usize].text.is_empty() {
                        d.nodes[c as usize].parent = None;
                        continue;
                    }
                    if let Some(&last) = keep.last() {
                        if d.nodes[last as usize].kind == TEXT_NODE {
                            let joined = alloc::format!("{}{}",
                                d.nodes[last as usize].text, d.nodes[c as usize].text);
                            d.nodes[last as usize].text = Rc::from(joined.as_str());
                            d.nodes[c as usize].parent = None;
                            continue;
                        }
                    }
                } else {
                    walk(d, c);
                }
                keep.push(c);
            }
            d.nodes[id as usize].children = keep;
        }
        if let Some(d) = &mut i.doc { walk(d, id); d.touch(); }
        Ok(Value::Undefined)
    }, 0, &fp);
    meth(&node_proto, "appendChild", |i, t, a| {
        let p = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.insert_maybe_fragment(p, c, None); }
        fire_connected(i, c)?;
        Ok(a[0].clone())
    }, 1, &fp);
    meth(&node_proto, "insertBefore", |i, t, a| {
        let p = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let b = match a.get(1) { Some(Value::Obj(_)) => Some(node_of(i, &a[1])?), _ => None };
        if let Some(d) = &mut i.doc { d.insert_maybe_fragment(p, c, b); }
        fire_connected(i, c)?;
        Ok(a[0].clone())
    }, 2, &fp);
    meth(&node_proto, "removeChild", |i, t, a| {
        let _ = node_of(i, &t)?;
        let c = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.detach(c); }
        Ok(a[0].clone())
    }, 1, &fp);
    // `replaceChild(new, old)`. Icon libraries such as lucide replace every
    // placeholder element with its `<svg>` this way.
    meth(&node_proto, "replaceChild", |i, t, a| {
        let p = node_of(i, &t)?;
        let new = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let old = node_of(i, a.get(1).unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc {
            if d.nodes[old as usize].parent != Some(p) {
                return i.type_err("replaceChild: the node is not a child of this node");
            }
            // Insert first, then remove: the other way round the position for the
            // new node would be gone and it would end up last.
            d.insert_maybe_fragment(p, new, Some(old));
            d.detach(old);
        }
        fire_connected(i, new)?;
        Ok(a[1].clone())
    }, 2, &fp);
    // Structural comparison (DOM §4.4): same node type, same name, same
    // attributes (set and values, order irrelevant) and the same children in
    // the same order. Not identity; that is `===`.
    meth(&node_proto, "isEqualNode", |i, t, a| {
        let x = node_of(i, &t)?;
        let Ok(y) = node_of(i, a.first().unwrap_or(&Value::Undefined)) else {
            return Ok(Value::Bool(false));
        };
        Ok(Value::Bool(i.doc.as_ref().is_some_and(|d| nodes_equal(d, x, y))))
    }, 1, &fp);
    accessor(&node_proto, "nodeValue",
        // An element has no value: the answer is `null`, not "".
        |i, t, _| with_node!(i, t, |n| Ok(if n.kind == ELEMENT_NODE || n.kind == DOCUMENT_NODE {
            Value::Null } else { Value::Str(n.text.clone()) })),
        |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc {
                if d.nodes[id as usize].kind != ELEMENT_NODE {
                    d.set_text(id, v);
                }
            }
            Ok(Value::Undefined)
        }, &fp);
    // The bitmask from the spec. Pages use it for one question, "is A before
    // B?", asked as `& 4`.
    meth(&node_proto, "compareDocumentPosition", |i, t, a| {
        let x = node_of(i, &t)?;
        let y = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if x == y { return Ok(Value::Num(0.0)) }
        let Some(d) = &i.doc else { return Ok(Value::Num(1.0)) };
        let up = |mut n: u32| { let mut v = alloc::vec![n];
            while let Some(p) = d.nodes[n as usize].parent { v.push(p); n = p; } v.reverse(); v };
        let (ax, ay) = (up(x), up(y));
        if ax[0] != ay[0] { return Ok(Value::Num(1.0 + 2.0 + 32.0)) }   // DISCONNECTED
        // The first point where the paths diverge decides.
        let mut k = 0;
        while k < ax.len() && k < ay.len() && ax[k] == ay[k] { k += 1; }
        if k == ax.len() { return Ok(Value::Num(16.0 + 4.0)) }          // CONTAINED_BY
        if k == ay.len() { return Ok(Value::Num(8.0 + 2.0)) }           // CONTAINS
        let parent = ax[k - 1];
        let kids = &d.nodes[parent as usize].children;
        let (px, py) = (kids.iter().position(|&c| c == ax[k]), kids.iter().position(|&c| c == ay[k]));
        Ok(Value::Num(if px < py { 4.0 } else { 2.0 }))
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
    // Register; dispatch happens elsewhere. Throwing here would end the
    // calling script.
    meth(&event_target_proto, "addEventListener", |i, t, a| {
        let id = target_node(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if let Some(d) = &mut i.doc {
            d.nodes[id as usize].listeners.push((ev, f));
            // As soon as one handler exists, layout needs hit boxes.
            d.has_listeners = true;
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    // `removeEventListener(type, f)` removes exactly f, not every listener of
    // that type.
    meth(&event_target_proto, "removeEventListener", |i, t, a| {
        let id = target_node(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if let Some(d) = &mut i.doc {
            d.nodes[id as usize].listeners.retain(|(e, g)| *e != ev || !same_fn(g, &f));
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    // The page dispatches itself: `el.dispatchEvent(new Event("change"))`.
    meth(&event_target_proto, "dispatchEvent", |i, t, a| {
        let id = target_node(i, &t)?;
        let Some(Value::Obj(ev)) = a.first().cloned() else {
            return i.type_err("dispatchEvent needs an Event");
        };
        let kind = match i.get(&Value::Obj(ev.clone()), "type")? {
            Value::Undefined => return i.type_err("dispatchEvent needs an Event"),
            v => i.to_string(&v)?,
        };
        let bubbles = matches!(i.get(&Value::Obj(ev.clone()), "bubbles")?, Value::Bool(true));
        // If it does not bubble, the path is one node long and only the target's
        // handler runs.
        let chain = if bubbles { ancestors(i, id) } else { alloc::vec![id] };
        let prevented = deliver(i, &ev, &kind, &chain)?;
        Ok(Value::Bool(!prevented))
    }, 1, &fp);

    // ── Element ──────────────────────────────────────────────────────────
    getter(&element_proto, "tagName", |i, t, _| with_node!(i, t, |n| Ok(Value::string(n.tag.to_ascii_uppercase()))), &fp);
    getter(&element_proto, "localName", |i, t, _| with_node!(i, t, |n| Ok(Value::Str(n.tag.clone()))), &fp);
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
                    if let Some(d) = &mut i.doc { d.set_attr_at(id, "id", &v); }
                    Ok(Value::Undefined) }, &fp);
    accessor(&element_proto, "className",
        |i, t, _| with_node!(i, t, |n| Ok(match n.attr("class") { Some(v) => Value::Str(v.clone()), None => Value::str("") })),
        |i, t, a| { let id = node_of(i, &t)?; let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                    if let Some(d) = &mut i.doc { d.set_attr_at(id, "class", &v); }
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
        if let Some(d) = &mut i.doc { d.set_attr_at(id, &k, &v); }
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&element_proto, "removeAttribute", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc { d.remove_attr_at(id, &k); }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&element_proto, "matches", |i, t, a| {
        let id = node_of(i, &t)?;
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let sel = parse_selector(i, &s)?;
        Ok(Value::Bool(sel_matches(i, &sel, id, Some(id))))
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
        // The four positions from the spec: before/after the element itself, and
        // at the very start/end inside it.
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
    // The same four positions, but with a node instead of a string.
    meth(&element_proto, "insertAdjacentElement", |i, t, a| {
        let id = node_of(i, &t)?;
        let pos = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_lowercase();
        let node = node_of(i, a.get(1).unwrap_or(&Value::Undefined))?;
        let Some((parent, before)) = adjacent_spot(i, id, &pos) else {
            return i.type_err("invalid insertAdjacentElement position");
        };
        if let Some(d) = &mut i.doc { d.insert_maybe_fragment(parent, node, before); }
        fire_connected(i, node)?;
        Ok(a[1].clone())
    }, 2, &fp);
    meth(&element_proto, "insertAdjacentText", |i, t, a| {
        let id = node_of(i, &t)?;
        let pos = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_lowercase();
        let text = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some((parent, before)) = adjacent_spot(i, id, &pos) else {
            return i.type_err("invalid insertAdjacentText position");
        };
        let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
        let n = d.create(TEXT_NODE, "#text");
        d.set_text(n, text);
        d.insert_maybe_fragment(parent, n, before);
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
    // ── Geometry ─────────────────────────────────────────────────────────
    //
    // Event handlers run on a laid-out page, so these answer from the layout
    // boxes. A wrong 0 would not throw or log anything; it would just look
    // like an answer.
    meth(&element_proto, "getBoundingClientRect", |i, t, _| {
        ensure_box(i, &t);
        let r = elem_rect(i, &t);
        Ok(Value::Obj(rect_obj(i, r)))
    }, 0, &fp);
    // The fragments individually: an inline box over three lines has three
    // rectangles, which is why this exists next to the function above.
    meth(&element_proto, "getClientRects", |i, t, _| {
        ensure_box(i, &t);
        let (sx, sy) = i.geometry.as_ref().map_or((0, 0), |g| g.scroll);
        let seq = layout_seq(i, &t);
        let rects: Vec<(f64, f64, f64, f64)> = match (&i.geometry, seq) {
            (Some(g), Some(seq)) => g.boxes.iter().filter(|b| b.seq == seq)
                .map(|b| ((b.x - sx) as f64, (b.y - sy) as f64, b.w as f64, b.h as f64)).collect(),
            _ => Vec::new(),
        };
        let out = new_obj(Some(i.realm.object_proto.clone()));
        for (n, r) in rects.iter().enumerate() {
            let o = rect_obj(i, Some(*r));
            out.borrow_mut().define(&alloc::format!("{n}"), Prop::data(Value::Obj(o)));
        }
        out.borrow_mut().define("length", Prop::data(Value::Num(rects.len() as f64)));
        Ok(Value::Obj(out))
    }, 0, &fp);
    getter(&element_proto, "offsetWidth",
        |i, t, _| { ensure_box(i, &t); Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.2))) }, &fp);
    getter(&element_proto, "offsetHeight",
        |i, t, _| { ensure_box(i, &t); Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.3))) }, &fp);
    // `offsetTop`/`offsetLeft` should be relative to the `offsetParent`; here
    // they are relative to the document. For the usual case (an element in an
    // unpositioned body) that is the same number. Not exact in general.
    getter(&element_proto, "offsetTop", |i, t, _| {
        ensure_box(i, &t);
        let sy = i.geometry.as_ref().map_or(0, |g| g.scroll.1);
        Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.1 + sy as f64)))
    }, &fp);
    getter(&element_proto, "offsetLeft", |i, t, _| {
        ensure_box(i, &t);
        let sx = i.geometry.as_ref().map_or(0, |g| g.scroll.0);
        Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.0 + sx as f64)))
    }, &fp);
    // `clientWidth`/`clientHeight` are the padding box: the border box minus
    // the borders. `HoverBox` carries the sums.
    //
    // Except on the root element, where they are the viewport (CSSOM View §4:
    // "If the element is the root element … return the viewport
    // width/height"). Pages measure the window this way.
    getter(&element_proto, "clientWidth", |i, t, _| {
        ensure_box(i, &t);
        if is_root_element(i, &t) { return Ok(viewport_num(i, "innerWidth")) }
        Ok(Value::Num(elem_inner(i, &t).map_or(0.0, |(w, _)| w)))
    }, &fp);
    getter(&element_proto, "clientHeight", |i, t, _| {
        ensure_box(i, &t);
        if is_root_element(i, &t) { return Ok(viewport_num(i, "innerHeight")) }
        Ok(Value::Num(elem_inner(i, &t).map_or(0.0, |(_, h)| h)))
    }, &fp);
    // ── Scroll metrics ───────────────────────────────────────────────────
    //
    // beak clips nothing: `overflow: auto`/`scroll` does not create scroll
    // containers per element. Therefore:
    //
    // * `scrollTop`/`scrollLeft` are 0 on every ordinary element, and that is
    //   true, since nothing in it is scrolled. On the root element and `<body>`
    //   they are the page's scroll position.
    // * `scrollHeight`/`scrollWidth` are the scrollable overflow: the padding
    //   box united with the border boxes of all descendants.
    // * On the root element it is the document's scrollable area as layout
    //   reports it (`Geometry::content`); a background or overflowing text can
    //   keep a page scrollable without having a box.
    getter(&element_proto, "scrollHeight", |i, t, _| {
        ensure_box(i, &t);
        if is_scrolling_root(i, &t) {
            let c = i.geometry.as_ref().map_or(0.0, |g| g.content.1 as f64);
            return Ok(Value::Num(c.max(viewport_f(i, "innerHeight"))));
        }
        Ok(Value::Num(scroll_area(i, &t).map_or(0.0, |(_, h)| h)))
    }, &fp);
    getter(&element_proto, "scrollWidth", |i, t, _| {
        ensure_box(i, &t);
        if is_scrolling_root(i, &t) {
            let c = i.geometry.as_ref().map_or(0.0, |g| g.content.0 as f64);
            return Ok(Value::Num(c.max(viewport_f(i, "innerWidth"))));
        }
        Ok(Value::Num(scroll_area(i, &t).map_or(0.0, |(w, _)| w)))
    }, &fp);
    accessor(&element_proto, "scrollTop",
        |i, t, _| {
            if !is_scrolling_root(i, &t) { return Ok(Value::Num(0.0)) }
            Ok(Value::Num(i.geometry.as_ref().map_or(0.0, |g| g.scroll.1 as f64)))
        },
        |i, t, a| {
            if !is_scrolling_root(i, &t) { return Ok(Value::Undefined) }
            let y = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
            i.want_scroll(None, Some(y));
            Ok(Value::Undefined)
        }, &fp);
    accessor(&element_proto, "scrollLeft",
        |i, t, _| {
            if !is_scrolling_root(i, &t) { return Ok(Value::Num(0.0)) }
            Ok(Value::Num(i.geometry.as_ref().map_or(0.0, |g| g.scroll.0 as f64)))
        },
        |i, t, a| {
            if !is_scrolling_root(i, &t) { return Ok(Value::Undefined) }
            let x = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
            i.want_scroll(Some(x), None);
            Ok(Value::Undefined)
        }, &fp);
    // `offsetParent`: the nearest positioned ancestor, else `<body>` (CSSOM
    // View §5). Whether a box is positioned travels with the layout box;
    // otherwise each ancestor's cascade would have to be resolved.
    //
    // `null` on an element without a box, on the root element and on `<body>`
    // itself. Pages check this to ask whether an element is visible at all.
    getter(&element_proto, "offsetParent", |i, t, _| {
        ensure_box(i, &t);
        let id = node_of(i, &t)?;
        let (html, body) = match &i.doc {
            Some(d) => (d.html, d.body),
            None => return Ok(Value::Null),
        };
        if Some(id) == html || Some(id) == body { return Ok(Value::Null) }
        // No box, no reference: `display: none`, which is the usual question.
        if node_box(i, id).is_none() { return Ok(Value::Null) }
        let mut cur = i.doc.as_ref().and_then(|d| d.nodes[id as usize].parent);
        while let Some(p) = cur {
            if Some(p) == body || Some(p) == html { break }
            if node_seq(i, p).is_some_and(|s| i.geometry.as_ref()
                .is_some_and(|g| g.boxes.iter().any(|b| b.seq == s && b.positioned))) {
                return Ok(wrap(i, p));
            }
            cur = i.doc.as_ref().and_then(|d| d.nodes[p as usize].parent);
        }
        Ok(match body { Some(b) => wrap(i, b), None => Value::Null })
    }, &fp);
    // Programmatic scrolling. The engine does not scroll; it records what the
    // page wanted and the host collects it with `take_scroll`. The engine has
    // no window and must not invent one.
    meth(&element_proto, "scrollTo", |i, t, a| {
        if !is_scrolling_root(i, &t) { return Ok(Value::Undefined) }
        let (x, y) = scroll_args(i, a)?;
        i.want_scroll(x, y);
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&element_proto, "scrollBy", |i, t, a| {
        if !is_scrolling_root(i, &t) { return Ok(Value::Undefined) }
        let (dx, dy) = scroll_args(i, a)?;
        let (sx, sy) = i.geometry.as_ref().map_or((0.0, 0.0),
            |g| (g.scroll.0 as f64, g.scroll.1 as f64));
        i.want_scroll(dx.map(|v| sx + v), dy.map(|v| sy + v));
        Ok(Value::Undefined)
    }, 2, &fp);
    // `scrollIntoView` scrolls so the element's top edge is at the top, the
    // spec default (`block: "start"`). `false` or `{ block: "end" }` puts the
    // bottom edge at the bottom.
    meth(&element_proto, "scrollIntoView", |i, t, a| {
        let Some((_, y, _, h)) = elem_rect(i, &t) else { return Ok(Value::Undefined) };
        let sy = i.geometry.as_ref().map_or(0.0, |g| g.scroll.1 as f64);
        let doc_y = y + sy;
        let ende = match a.first() {
            Some(Value::Bool(false)) => true,
            Some(o @ Value::Obj(_)) => matches!(i.get(o, "block")?,
                Value::Str(s) if &*s == "end" || &*s == "nearest"),
            _ => false,
        };
        let ziel = if ende { doc_y + h - viewport_f(i, "innerHeight") } else { doc_y };
        i.want_scroll(None, Some(ziel.max(0.0)));
        Ok(Value::Undefined)
    }, 1, &fp);
    // The list works on the element: it holds no copy of the classes but
    // reads and writes the attribute. Built fresh per access, so
    // `el.classList === el.classList` is false, while browsers return the
    // same object.
    getter(&element_proto, "classList", |i, t, _| {
        let id = node_of(i, &t)?;
        let g = new_obj(Some(i.realm.token_list_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, &fp);
    // `el.style` is a view on this element's `style` attribute; it holds no
    // state of its own, so attribute and view cannot diverge.
    getter(&element_proto, "style", |i, t, _| {
        let id = node_of(i, &t)?;
        let g = new_obj(Some(i.realm.style_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, &fp);

    // querySelector and friends, on Element as on Document.
    // `append`, `prepend`, `before`, `after`, `replaceWith`: the modern
    // insertion family. Takes any number of arguments, and a string becomes
    // a text node.
    for target in [&element_proto, &fragment_proto] {
        meth(target, "append", |i, t, a| insert_all(i, &t, a, Where::Last), 1, &fp);
        meth(target, "prepend", |i, t, a| insert_all(i, &t, a, Where::First), 1, &fp);
    }
    meth(&element_proto, "before", |i, t, a| insert_all(i, &t, a, Where::Before), 1, &fp);
    meth(&element_proto, "after", |i, t, a| insert_all(i, &t, a, Where::After), 1, &fp);
    meth(&element_proto, "replaceWith", |i, t, a| {
        let id = node_of(i, &t)?;
        insert_all(i, &t, a, Where::Before)?;
        if let Some(d) = &mut i.doc { d.detach(id); d.touch(); }
        Ok(Value::Undefined)
    }, 1, &fp);

    // Also on the fragment: a template is filled by querying its content.
    for target in [&element_proto, &document_proto, &fragment_proto] {
        meth(target, "querySelector", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let sel = parse_selector(i, &s)?;
            let found = sel_query(i, &sel, id, true);
            Ok(match found.first() { Some(&x) => wrap(i, x), None => Value::Null })
        }, 1, &fp);
        meth(target, "querySelectorAll", |i, t, a| {
            let id = node_of(i, &t)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let sel = parse_selector(i, &s)?;
            let found = sel_query(i, &sel, id, false);
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
    //
    // These getters read `this`, not just the main document:
    // `implementation.createHTMLDocument` puts a second document node in the
    // same arena, and returning the main body would let a library write its
    // probe into the real page. The main document keeps the cached path.
    getter(&document_proto, "documentElement", |i, this, _| {
        match doc_part(i, &this, DocPart::Root)? { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "body", |i, this, _| {
        match doc_part(i, &this, DocPart::Body)? { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "head", |i, this, _| {
        match doc_part(i, &this, DocPart::Head)? { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    getter(&document_proto, "readyState", |_, _, _| Ok(Value::str("complete")), &fp);
    // `currentScript` is how a bundle finds itself (bundlers push
    // `document.currentScript` with each chunk). It also tells
    // `document.write` where to write.
    //
    // Only on the main document and only while a classic script runs; in a
    // module, a callback or another document it is `null` (HTML §4.12.1).
    getter(&document_proto, "currentScript", |i, this, _| {
        let root = node_of(i, &this)?;
        if i.doc.as_ref().map(|d| d.doc) != Some(root) { return Ok(Value::Null) }
        match i.current_script { Some(n) => Ok(wrap(i, n)), None => Ok(Value::Null) }
    }, &fp);
    // `scrollingElement`: the element whose `scrollTop` scrolls the page. In
    // standards mode that is `documentElement`, and beak parses nothing else.
    // Must agree with `is_scrolling_root`.
    getter(&document_proto, "scrollingElement", |i, this, _| {
        match doc_part(i, &this, DocPart::Root)? { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    // `document.write`: in browsers a classic script runs during parsing and
    // writes into the stream; in beak the tree is complete before the first
    // script runs. The result is the same if the text goes where the parser
    // would be: right after the writing `<script>`.
    //
    // Per spec a `write` without an insertion point is a `document.open()`,
    // which clears the document. beak does not do that, since no script here
    // ever has a spec insertion point and an empty page is the worse answer.
    // Without `currentScript` (from a timer or callback) it logs to the
    // console and does nothing.
    //
    // `visibilityState`/`hidden`: beak paints one document per session, and
    // background tabs are frozen without a JS session, so any page asking is
    // visible. If background tabs ever stay live, these getters and
    // `visibilitychange` must change with it.
    meth(&document_proto, "write", |i, this, a| doc_write(i, &this, a, false), 1, &fp);
    meth(&document_proto, "writeln", |i, this, a| doc_write(i, &this, a, true), 1, &fp);
    getter(&document_proto, "visibilityState", |_, _, _| Ok(Value::str("visible")), &fp);
    getter(&document_proto, "hidden", |_, _, _| Ok(Value::Bool(false)), &fp);
    meth(&document_proto, "hasFocus", |_, _, _| Ok(Value::Bool(true)), 0, &fp);
    // `referrer` is the empty string, the correct answer for a navigation
    // without a referrer; beak does not pass one on yet.
    getter(&document_proto, "referrer", |_, _, _| Ok(Value::str("")), &fp);
    // `document.domain` (HTML §7.5.2): the host of the document's origin, the
    // empty string for an opaque origin. Setting it is only accepted when
    // nothing changes; relaxing to a parent domain is deprecated and not
    // supported.
    accessor(&document_proto, "domain",
        |i, _, _| {
            let host = super::url::parse_abs(&i.loc_href)
                .filter(|p| matches!(p.scheme.as_str(), "http" | "https"))
                .map(|p| p.host).unwrap_or_default();
            Ok(Value::string(host))
        },
        |i, _, a| {
            let want = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let host = super::url::parse_abs(&i.loc_href).map(|p| p.host).unwrap_or_default();
            if want.eq_ignore_ascii_case(&host) { return Ok(Value::Undefined) }
            Err(dom_exc(i, "SecurityError", "document.domain cannot be changed"))
        }, &fp);
    // `document.cookie`. The engine holds no cookie jar: what this document
    // may see depends on domain, path, `Secure` and `HttpOnly`, which the host
    // knows; `Interp::set_cookies` provides the script view. Writes go back
    // the same way (`take_cookie_sets`).
    accessor(&document_proto, "cookie",
        |i, _, _| { let c = i.cookies.clone(); Ok(Value::str(&c)) },
        |i, _, a| {
            let decl = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let Some((name, rest)) = decl.split_once('=') else { return Ok(Value::Undefined) };
            let name = name.trim().to_string();
            if name.is_empty() { return Ok(Value::Undefined) }
            let value = rest.split(';').next().unwrap_or("").trim().to_string();
            // The engine recognises deletion only by `Max-Age<=0`, which needs no
            // clock. An `Expires` in the past needs a clock the engine lacks; that
            // case shows once the host provides the view again. The jar itself
            // handles both.
            let deleting = decl.split(';').skip(1).any(|a| {
                let (k, v) = a.split_once('=').unwrap_or((a, ""));
                k.trim().eq_ignore_ascii_case("max-age")
                    && v.trim().parse::<i64>().is_ok_and(|n| n <= 0)
            });
            let mut kept: Vec<String> = i.cookies.split(';')
                .map(|p| p.trim()).filter(|p| !p.is_empty())
                .filter(|p| p.split_once('=').map(|(k, _)| k.trim()) != Some(&name[..]))
                .map(|p| p.to_string()).collect();
            if !deleting { kept.push(alloc::format!("{name}={value}")); }
            i.cookies = kept.join("; ");
            i.cookie_sets.push(decl.to_string());
            Ok(Value::Undefined)
        }, &fp);
    // The title lives in the tree: setting it changes the `<title>` element.
    // It reads and writes this document node's own title, so a new
    // `createHTMLDocument("Title")` does not return or overwrite the real
    // page's title.
    accessor(&document_proto, "title",
        |i, this, _| {
            let root = node_of(i, &this)?;
            let Some(d) = i.doc.as_ref() else { return Ok(Value::str("")) };
            Ok(match d.find_tag(root, "title") {
                Some(x) => Value::str(&d.text_of(x)),
                None => Value::str(""),
            })
        },
        |i, this, a| {
            let root = node_of(i, &this)?;
            let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let found = i.doc.as_ref().and_then(|d| d.find_tag(root, "title"));
            let head = doc_part(i, &this, DocPart::Head)?;
            let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
            let t = match found {
                Some(x) => x,
                // No `<title>`: create one and append it to the head.
                None => {
                    let e = d.create(ELEMENT_NODE, "title");
                    d.append(head.unwrap_or(root), e);
                    e
                }
            };
            d.clear_children(t);
            let tn = d.create(TEXT_NODE, "");
            d.nodes[tn as usize].text = s;
            d.append(t, tn);
            d.touch();
            Ok(Value::Undefined)
        }, &fp);
    // Commonly used Node/Element/Document members.
    getter(&node_proto, "ownerDocument", |i, t, _| {
        let id = node_of(i, &t)?;
        let Some(d) = &i.doc else { return Ok(Value::Null) };
        let root = d.doc;
        if id == root { return Ok(Value::Null) }              // the document itself: null
        Ok(wrap(i, root))
    }, &fp);
    meth(&node_proto, "getRootNode", |i, t, _| {
        let mut id = node_of(i, &t)?;
        loop {
            let Some(d) = &i.doc else { return Ok(Value::Null) };
            match d.nodes[id as usize].parent { Some(p) => id = p, None => break }
        }
        Ok(wrap(i, id))
    }, 0, &fp);
    getter(&element_proto, "namespaceURI", |i, t, _| {
        // Only SVG and HTML are distinguished; `foreignObject` content would get
        // the wrong answer.
        with_node!(i, t, |n| Ok(Value::str(
            if &*n.tag == "svg" || n.tag.starts_with("svg:") { "http://www.w3.org/2000/svg" }
            else { "http://www.w3.org/1999/xhtml" })))
    }, &fp);
    meth(&element_proto, "closest", |i, t, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let sel = parse_selector(i, &s)?;
        let me = node_of(i, &t)?;
        let mut id = Some(me);
        while let Some(x) = id {
            let hit = sel_matches(i, &sel, x, Some(me));
            if hit { return Ok(wrap(i, x)) }
            let Some(d) = &i.doc else { break };
            id = d.nodes[x as usize].parent;
        }
        Ok(Value::Null)
    }, 1, &fp);
    meth(&element_proto, "getAttributeNames", |i, t, _| {
        let names: Vec<Value> = with_node!(i, t, |n|
            n.attrs.iter().map(|(k, _)| Value::str(k)).collect::<Vec<_>>());
        Ok(i.new_array(names))
    }, 0, &fp);
    meth(&element_proto, "hasAttributes", |i, t, _| {
        with_node!(i, t, |n| Ok(Value::Bool(!n.attrs.is_empty())))
    }, 0, &fp);
    getter(&element_proto, "dataset", |i, t, _| {
        // Live on write, snapshot on read. The values are plain properties (reads
        // are the common case and should cost nothing), and `ObjKind::Dataset`
        // carries the node so `Interp::set` forwards an assignment to the
        // attribute.
        //
        // Not implemented: `delete el.dataset.x` does not remove the attribute,
        // and the object does not reflect attribute changes made elsewhere.
        let id = node_of(i, &t)?;
        let pairs: Vec<(String, String)> = with_node!(i, t, |n|
            n.attrs.iter().filter_map(|(k, v)| k.strip_prefix("data-")
                .map(|r| (dash_to_camel(r), v.to_string()))).collect::<Vec<_>>());
        let g = new_kind(Some(i.realm.object_proto.clone()), ObjKind::Dataset(id));
        for (k, v) in pairs { g.borrow_mut().define(&k, Prop::data(Value::string(v))); }
        Ok(Value::Obj(g))
    }, &fp);
    getter(&document_proto, "defaultView", |i, _, _| {
        Ok(Value::Obj(i.realm.global.clone()))
    }, &fp);
    // `document.forms`, with named access: `document.forms["loginForm"]`
    // looks up by `id` and `name`.
    getter(&document_proto, "forms", |i, _, _| {
        let ids = match &i.doc { Some(d) => tags_of(d, d.doc, "form"), None => Vec::new() };
        let arr = nodes_array(i, ids.clone());
        if let Value::Obj(o) = &arr {
            for id in ids {
                let (name, ident) = match &i.doc {
                    Some(d) => (d.nodes[id as usize].attr("name").cloned(),
                                d.nodes[id as usize].attr("id").cloned()),
                    None => (None, None),
                };
                let v = wrap(i, id);
                for k in [name, ident].into_iter().flatten() {
                    if k.is_empty() { continue }
                    o.borrow_mut().define(&k, Prop {
                        value: Some(v.clone()), get: None, set: None,
                        writable: true, enumerable: false, configurable: true });
                }
            }
        }
        Ok(arr)
    }, &fp);
    getter(&document_proto, "activeElement", |i, _, _| {
        // What `focus()` set, else `body`, the answer browsers give without
        // focus.
        if let Some(f) = i.doc.as_ref().and_then(|d| d.focused) { return Ok(wrap(i, f)) }
        let b = i.doc.as_ref().and_then(|d| find_tag(d, "body"));
        Ok(match b { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    meth(&document_proto, "createComment", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(COMMENT_NODE, "#comment");
        d.nodes[id as usize].text = s;
        Ok(wrap(i, id))
    }, 1, &fp);
    // `document.createEvent` (DOM Level 2), still used by shipped code. Builds
    // the spec's name table for the interfaces that exist here, and throws as
    // DOM §createEvent specifies for any other.
    meth(&document_proto, "createEvent", |i, _, a| {
        let want = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_lowercase();
        let custom = want == "customevent";
        // The table is case-insensitive; the plural names are older spellings of
        // the same entry.
        if !custom && !matches!(&*want, "event" | "events" | "htmlevents" | "svgevents") {
            return i.type_err(&alloc::format!(
                "createEvent: die Schnittstelle '{want}' gibt es in dieser Engine nicht"));
        }
        let proto = if custom {
            match i.get(&Value::Obj(i.realm.global.clone()), "CustomEvent")
                   .and_then(|c| i.get(&c, "prototype")) {
                Ok(Value::Obj(o)) => o, _ => i.realm.event_proto.clone(),
            }
        } else {
            i.realm.event_proto.clone()
        };
        // Created this way it is not initialized: the type stays empty until
        // `initEvent` sets it.
        Ok(Value::Obj(build_event(i, proto, "", false)))
    }, 1, &fp);
    meth(&document_proto, "createElementNS", |i, _, a| {
        let s = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        let lower = s.to_lowercase();
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &lower);
        Ok(wrap(i, id))
    }, 2, &fp);

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
        let lower = s.to_ascii_lowercase();
        if let Some(k) = i.custom.by_name(&lower) { return ce_create_sync(i, k); }
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &lower);
        Ok(wrap(i, id))
    }, 1, &fp);
    // `new Image()` is `document.createElement("img")` under another name, so
    // it builds a real `img` element (the usual use is a tracking pixel via
    // `new Image().src = …`).
    let img_ctor = native(Some(fp.clone()), |i, _, a| {
        let node = {
            let Some(d) = &mut i.doc else { return i.type_err("no document") };
            d.create(ELEMENT_NODE, "img")
        };
        let el = wrap(i, node);
        // `new Image(w, h)` sets width and height as attributes, as in browsers,
        // not as style.
        for (k, n) in [("width", 0usize), ("height", 1usize)] {
            if let Some(v) = a.get(n) {
                if !matches!(v, Value::Undefined) {
                    let t = i.to_string(v)?;
                    if let Some(d) = &mut i.doc {
                        d.nodes[node as usize].attrs.push((Rc::from(k), t));
                    }
                }
            }
        }
        Ok(el)
    }, "Image", 0, true);
    realm.global.borrow_mut().define("Image", Prop::builtin(Value::Obj(img_ctor)));

    meth(&document_proto, "createTextNode", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(TEXT_NODE, "#text");
        d.nodes[id as usize].text = s;
        Ok(wrap(i, id))
    }, 1, &fp);
    // `importNode` and `adoptNode` both bring a node into this document. beak
    // has one node store, so `importNode` is a copy and `adoptNode` the node
    // itself, which is what the spec gives for that case.
    meth(&document_proto, "importNode", |i, _, a| {
        let id = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let deep = a.get(1).map(|v| v.truthy()).unwrap_or(false);
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let c = d.clone_node(id, deep);
        Ok(wrap(i, c))
    }, 1, &fp);
    meth(&document_proto, "adoptNode", |i, _, a| {
        let id = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        Ok(wrap(i, id))
    }, 1, &fp);
    // ── MutationObserver ─────────────────────────────────────────────────
    //
    // Libraries that bring loaded HTML to life (Alpine, htmx) observe the
    // tree rather than an event.
    //
    // Recorded in the tree (`Doc::record`), delivered at the microtask
    // checkpoint (`promise::run_jobs`). Records are built only on delivery,
    // not per mutation while the script runs.
    let mo_proto = new_obj(Some(realm.object_proto.clone()));
    let mo_ctor = native(Some(fp.clone()), |i, _, a| {
        let cb = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&cb) {
            return i.type_err("MutationObserver: argument 1 is not a function");
        }
        let o = new_obj(Some(i.realm.mo_proto.clone()));
        i.observers.push(MutObs { js: o.clone(), cb, regs: Vec::new(), queue: Vec::new() });
        Ok(Value::Obj(o))
    }, "MutationObserver", 1, true);
    mo_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(mo_proto.clone())));
    mo_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(mo_ctor.clone())));
    mo_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("MutationObserver")));
    realm.global.borrow_mut().define("MutationObserver", Prop::builtin(Value::Obj(mo_ctor)));
    realm.mo_proto = mo_proto.clone();

    meth(&mo_proto, "observe", |i, t, a| {
        let target = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
        let has_opts = matches!(opts, Value::Obj(_));
        let mut flag = |i: &mut Interp, n: &str| -> C<bool> {
            if !has_opts { return Ok(false) }
            Ok(i.get(&opts, n)?.truthy())
        };
        let child_list = flag(i, "childList")?;
        let subtree = flag(i, "subtree")?;
        let char_data = flag(i, "characterData")?;
        let attr_old = flag(i, "attributeOldValue")?;
        let char_old = flag(i, "characterDataOldValue")?;
        let explicit_attrs = flag(i, "attributes")?;
        // `attributeFilter` implies `attributes` even if not written, as the spec
        // says; library code relies on it.
        let filter = if !has_opts { None } else {
            match i.get(&opts, "attributeFilter")? {
                Value::Obj(_) => {
                    let fv = i.get(&opts, "attributeFilter")?;
                    let n = match i.get(&fv, "length") { Ok(Value::Num(n)) => n as usize, _ => 0 };
                    let mut v: Vec<Rc<str>> = Vec::with_capacity(n);
                    for k in 0..n {
                        let e = i.get(&fv, &alloc::format!("{k}"))?;
                        let sv = i.to_string(&e)?;
                        v.push(sv);
                    }
                    Some(v)
                }
                _ => None,
            }
        };
        let attrs = explicit_attrs || attr_old || filter.is_some();
        // Without one of the three there is nothing to observe; the spec throws
        // here rather than creating an observer that never reports.
        if !child_list && !attrs && !char_data {
            return i.type_err("MutationObserver.observe: childList, attributes or characterData required");
        }
        let Value::Obj(this) = &t else { return i.type_err("MutationObserver.observe: not an observer") };
        let reg = MutReg { target, slot: usize::MAX, subtree, child_list, attrs,
                           attr_old, char_data, char_old, filter };
        let mut found = false;
        for o in i.observers.iter_mut() {
            if Rc::ptr_eq(&o.js, this) {
                // A second `observe` on the same node replaces the registration rather
                // than adding one (DOM §4.3.1).
                o.regs.retain(|r| r.target != target);
                o.regs.push(reg);
                found = true;
                break;
            }
        }
        if !found { return i.type_err("MutationObserver.observe: not an observer") }
        sync_observing(i);
        Ok(Value::Undefined)
    }, 2, &fp);

    meth(&mo_proto, "disconnect", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(Value::Undefined) };
        for o in i.observers.iter_mut() {
            if Rc::ptr_eq(&o.js, this) { o.regs.clear(); o.queue.clear(); }
        }
        sync_observing(i);
        Ok(Value::Undefined)
    }, 0, &fp);

    meth(&mo_proto, "takeRecords", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(i.new_array(Vec::new())) };
        let this = this.clone();
        // First collect what the tree recorded since last time, otherwise
        // `takeRecords()` right after a change would return an empty list.
        collect_mutations(i);
        let mut recs = Vec::new();
        for o in i.observers.iter_mut() {
            if Rc::ptr_eq(&o.js, &this) { recs = core::mem::take(&mut o.queue); }
        }
        let mut vals = Vec::with_capacity(recs.len());
        for m in &recs { vals.push(build_record(i, m)?); }
        Ok(i.new_array(vals))
    }, 0, &fp);


    // ── ResizeObserver ───────────────────────────────────────────────────
    //
    // For components that adapt to their own box; the window's `resize` says
    // nothing about one box changing because something next to it collapsed.
    let ro_proto = new_obj(Some(realm.object_proto.clone()));
    let ro_ctor = native(Some(fp.clone()), |i, _, a| {
        let cb = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&cb) {
            return i.type_err("ResizeObserver: argument 1 is not a function");
        }
        let o = new_obj(Some(i.realm.ro_proto.clone()));
        i.resize_obs.push(ResizeObs { js: o.clone(), cb, regs: Vec::new(), queue: Vec::new() });
        Ok(Value::Obj(o))
    }, "ResizeObserver", 1, true);
    ro_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(ro_proto.clone())));
    ro_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ro_ctor.clone())));
    ro_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("ResizeObserver")));
    realm.global.borrow_mut().define("ResizeObserver", Prop::builtin(Value::Obj(ro_ctor)));
    realm.ro_proto = ro_proto.clone();

    meth(&ro_proto, "observe", |i, t, a| {
        let target = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        // `{ box: "border-box" }`; the default is the content box.
        let kind = match a.get(1) {
            Some(o @ Value::Obj(_)) => match i.get(o, "box")? {
                Value::Str(s) if &*s == "border-box" => BoxKind::Border,
                _ => BoxKind::Content,
            },
            _ => BoxKind::Content,
        };
        let Value::Obj(this) = &t else {
            return i.type_err("ResizeObserver.observe: not an observer")
        };
        let this = this.clone();
        let mut found = false;
        for o in i.resize_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, &this) {
                // A second `observe` on the same node replaces the registration (Resize
                // Observer 3.1), and `last: None` makes it report once afterwards, as
                // the first time.
                o.regs.retain(|r| r.target != target);
                o.regs.push(ResizeReg { target, kind, last: None });
                found = true;
                break;
            }
        }
        if !found { return i.type_err("ResizeObserver.observe: not an observer") }
        // Evaluate now: `observe` delivers the current size, not only the next
        // change. Waiting for the next layout could leave a page with no size
        // that never changes again.
        eval_box_observers(i);
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&ro_proto, "unobserve", |i, t, a| {
        let target = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let Value::Obj(this) = &t else { return Ok(Value::Undefined) };
        for o in i.resize_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, this) {
                o.regs.retain(|r| r.target != target);
                o.queue.retain(|e| e.target != target);
            }
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&ro_proto, "disconnect", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(Value::Undefined) };
        for o in i.resize_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, this) { o.regs.clear(); o.queue.clear(); }
        }
        Ok(Value::Undefined)
    }, 0, &fp);

    // ── IntersectionObserver ─────────────────────────────────────────────
    //
    // Lazy-loaded images, infinite lists, in-view animations and view
    // counting depend on it.
    let io_proto = new_obj(Some(realm.object_proto.clone()));
    let io_ctor = native(Some(fp.clone()), |i, _, a| {
        let cb = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&cb) {
            return i.type_err("IntersectionObserver: argument 1 is not a function");
        }
        let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
        let has = matches!(opts, Value::Obj(_));
        // The root: an element, or the viewport.
        let root = if !has { None } else {
            match i.get(&opts, "root")? {
                v @ Value::Obj(_) => node_of(i, &v).ok(),
                _ => None,
            }
        };
        // `rootMargin`: the CSS shorthand with one to four lengths. Percentages
        // refer to the clip size; without an explicit root that is the viewport.
        let (vw, vh) = i.viewport;
        let margin_src: Rc<str> = if !has { Rc::from("0px") } else {
            match i.get(&opts, "rootMargin")? {
                Value::Str(s) => s.clone(),
                Value::Undefined => Rc::from("0px"),
                v => i.to_string(&v)?,
            }
        };
        let margin = parse_root_margin(&margin_src, vw, vh);
        // `threshold`: a number or a list. Default 0, i.e. as soon as one pixel
        // becomes visible.
        let mut thresholds: Vec<f64> = Vec::new();
        if has {
            match i.get(&opts, "threshold")? {
                Value::Undefined | Value::Null => {}
                v @ Value::Obj(_) => {
                    let lv = i.get(&v, "length")?;
                    let len = match lv { Value::Num(n) if n >= 0.0 => n as usize, _ => 0 };
                    for k in 0..len {
                        let e = i.get(&v, &alloc::format!("{k}"))?;
                        thresholds.push(i.to_number(&e)?);
                    }
                }
                v => thresholds.push(i.to_number(&v)?),
            }
        }
        if thresholds.is_empty() { thresholds.push(0.0); }
        for t in thresholds.iter() {
            if !(*t >= 0.0 && *t <= 1.0) {
                return i.range_err("IntersectionObserver: threshold outside [0, 1]");
            }
        }
        thresholds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let o = new_obj(Some(i.realm.io_proto.clone()));
        i.inter_obs.push(InterObs { js: o.clone(), cb, root, margin_src, margin,
                                    thresholds, regs: Vec::new(), queue: Vec::new() });
        Ok(Value::Obj(o))
    }, "IntersectionObserver", 1, true);
    io_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(io_proto.clone())));
    io_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(io_ctor.clone())));
    io_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("IntersectionObserver")));
    realm.global.borrow_mut().define("IntersectionObserver", Prop::builtin(Value::Obj(io_ctor)));
    realm.io_proto = io_proto.clone();

    meth(&io_proto, "observe", |i, t, a| {
        let target = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let Value::Obj(this) = &t else {
            return i.type_err("IntersectionObserver.observe: not an observer")
        };
        let this = this.clone();
        let mut found = false;
        for o in i.inter_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, &this) {
                o.regs.retain(|r| r.target != target);
                o.regs.push(InterReg { target, band: -1 });
                found = true;
                break;
            }
        }
        if !found { return i.type_err("IntersectionObserver.observe: not an observer") }
        // As with `ResizeObserver`, the first report comes immediately, not on
        // the next scroll; lists already half visible at load rely on it.
        eval_box_observers(i);
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&io_proto, "unobserve", |i, t, a| {
        let target = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let Value::Obj(this) = &t else { return Ok(Value::Undefined) };
        for o in i.inter_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, this) {
                o.regs.retain(|r| r.target != target);
                o.queue.retain(|e| e.target != target);
            }
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&io_proto, "disconnect", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(Value::Undefined) };
        for o in i.inter_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, this) { o.regs.clear(); o.queue.clear(); }
        }
        Ok(Value::Undefined)
    }, 0, &fp);
    meth(&io_proto, "takeRecords", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(i.new_array(Vec::new())) };
        let this = this.clone();
        let mut q = Vec::new();
        for o in i.inter_obs.iter_mut() {
            if Rc::ptr_eq(&o.js, &this) { q = core::mem::take(&mut o.queue); }
        }
        let vals: Vec<Value> = q.iter().map(|e| build_inter_entry(i, e)).collect();
        Ok(i.new_array(vals))
    }, 0, &fp);
    // `root`, `rootMargin` and `thresholds` are readable; library code reads
    // them back to reuse an observer.
    getter(&io_proto, "root", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(Value::Null) };
        let r = i.inter_obs.iter().find(|o| Rc::ptr_eq(&o.js, this)).and_then(|o| o.root);
        Ok(match r { Some(id) => wrap(i, id), None => Value::Null })
    }, &fp);
    getter(&io_proto, "rootMargin", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(Value::str("0px")) };
        Ok(i.inter_obs.iter().find(|o| Rc::ptr_eq(&o.js, this))
            .map(|o| Value::Str(o.margin_src.clone()))
            .unwrap_or_else(|| Value::str("0px")))
    }, &fp);
    getter(&io_proto, "thresholds", |i, t, _| {
        let Value::Obj(this) = &t else { return Ok(i.new_array(Vec::new())) };
        let v: Vec<Value> = i.inter_obs.iter().find(|o| Rc::ptr_eq(&o.js, this))
            .map(|o| o.thresholds.iter().map(|x| Value::Num(*x)).collect())
            .unwrap_or_default();
        Ok(i.new_array(v))
    }, &fp);

    // ── document.implementation ──────────────────────────────────────────
    //
    // `createHTMLDocument` gives libraries (jQuery) a throwaway document for
    // parsing foreign HTML without loading images or running scripts.
    //
    // A second document node in the same node store, not a second store: node
    // ids are slots in exactly one `Vec`. It is detached, so neither layout
    // nor the host sees it.
    let impl_obj = new_obj(Some(realm.object_proto.clone()));
    meth(&impl_obj, "createHTMLDocument", |i, _, a| {
        let title = match a.first() {
            None | Some(Value::Undefined) => None,
            Some(v) => Some(i.to_string(v)?),
        };
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let doc = d.create(DOCUMENT_NODE, "#document");
        let html = d.create(ELEMENT_NODE, "html");
        let head = d.create(ELEMENT_NODE, "head");
        let body = d.create(ELEMENT_NODE, "body");
        d.append(doc, html);
        d.append(html, head);
        d.append(html, body);
        // `createHTMLDocument()` without an argument gets no title, which differs
        // from the empty title `("")` asks for.
        if let Some(t) = title {
            let el = d.create(ELEMENT_NODE, "title");
            let tx = d.create(TEXT_NODE, "#text");
            d.nodes[tx as usize].text = t;
            d.append(el, tx);
            d.append(head, el);
        }
        Ok(wrap(i, doc))
    }, 1, &fp);
    // `hasFeature` always returns true per spec.
    meth(&impl_obj, "hasFeature", |_, _, _| Ok(Value::Bool(true)), 0, &fp);
    // `createDocument` is the XML twin: a document node with exactly one root
    // element, no `head`, no `body`. The namespace is read and dropped, since
    // beak's tree has no namespaces.
    meth(&impl_obj, "createDocument", |i, _, a| {
        let qname = match a.get(1) {
            None | Some(Value::Undefined) | Some(Value::Null) => None,
            Some(v) => { let s = i.to_string(v)?; if s.is_empty() { None } else { Some(s) } }
        };
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let doc = d.create(DOCUMENT_NODE, "#document");
        if let Some(q) = qname {
            let root = d.create(ELEMENT_NODE, &q.to_lowercase());
            d.append(doc, root);
        }
        Ok(wrap(i, doc))
    }, 3, &fp);
    impl_obj.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("DOMImplementation")));
    document_proto.borrow_mut().define("implementation", Prop::builtin(Value::Obj(impl_obj)));

    // `reportError(e)` (HTML §8.1.3.9): report an error as an uncaught
    // exception would, to the window and then to the console. Frameworks
    // (React) report uncaught render errors through it; without it a page
    // renders nothing and says nothing.
    def_global(realm, "reportError", |i, _, a| {
        let err = a.first().cloned().unwrap_or(Value::Undefined);
        let msg = match i.get(&err, "message") {
            Ok(Value::Undefined) | Err(_) => i.to_string(&err).map(|s| s.to_string()).unwrap_or_default(),
            Ok(v) => i.to_string(&v).map(|s| s.to_string()).unwrap_or_default(),
        };
        let name = match i.get(&err, "name") {
            Ok(Value::Undefined) | Err(_) => alloc::string::String::new(),
            Ok(v) => i.to_string(&v).map(|s| s.to_string()).unwrap_or_default(),
        };
        let target = i.doc.as_ref().map(|d| d.doc);
        let mut handled = false;
        if let Some(t) = target {
            let proto = i.realm.event_proto.clone();
            let ev = build_event(i, proto, "error", false);
            ev.borrow_mut().define("message", Prop::data(Value::string(msg.clone())));
            ev.borrow_mut().define("error", Prop::data(err));
            handled = deliver(i, &ev, "error", &[t]).unwrap_or(false);
        }
        // `preventDefault` means "handled": the console stays silent, as in
        // browsers.
        if !handled {
            i.console_push(if name.is_empty() { alloc::format!("error: {msg}") }
                           else { alloc::format!("error: {name}: {msg}") });
        }
        Ok(Value::Undefined)
    }, 1, &fp);

    // ── CSS ──────────────────────────────────────────────────────────────
    //
    // A missing feature test is not neutral, it is a "no": pages ask
    // `CSS.supports` whether they may take the modern path, and otherwise
    // load polyfills (e.g. a CSS variables ponyfill that reparses every
    // stylesheet).
    //
    // Answered by the same function that evaluates `@supports` in
    // stylesheets.
    let css_obj = new_obj(Some(realm.object_proto.clone()));
    meth(&css_obj, "supports", |i, _, a| {
        let first = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        match a.get(1) {
            // The two-argument form takes name and value and returns false for a
            // custom property (css-conditional-3 §6); testing `--x` needs the
            // condition form.
            Some(v) => {
                let val = i.to_string(v)?;
                if first.trim().starts_with("--") { return Ok(Value::Bool(false)) }
                if val.trim().is_empty() { return Ok(Value::Bool(false)) }
                Ok(Value::Bool(crate::css::supports_decl(first.trim(), val.trim())))
            }
            None => Ok(Value::Bool(crate::css::supports_cond(&first))),
        }
    }, 2, &fp);
    // `CSS.escape` (cssom-1 §9): an identifier that may appear in a selector.
    // Libraries build `#\31 23` selectors from foreign ids with it.
    meth(&css_obj, "escape", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut out = String::new();
        for (k, c) in s.chars().enumerate() {
            let code = c as u32;
            let ok = c == '-' || c == '_' || c.is_ascii_alphanumeric() || code >= 0x80;
            if code == 0 { out.push('\u{FFFD}'); continue }
            // A leading digit, also after a leading `-`, must be written as a code
            // point, otherwise the parser reads a number.
            let lead_digit = c.is_ascii_digit()
                && (k == 0 || (k == 1 && s.starts_with('-')));
            if (code < 0x20 || code == 0x7F) || lead_digit {
                out.push('\\');
                push_hex(&mut out, code);
                out.push(' ');
                continue;
            }
            if k == 0 && c == '-' && s.chars().count() == 1 { out.push('\\'); out.push(c); continue }
            if ok { out.push(c) } else { out.push('\\'); out.push(c) }
        }
        Ok(Value::string(out))
    }, 1, &fp);
    css_obj.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CSS")));
    realm.global.borrow_mut().define("CSS", Prop::builtin(Value::Obj(css_obj)));

    // ── DOMParser ────────────────────────────────────────────────────────
    //
    // The safe way to read foreign HTML: `parseFromString(s, "text/html")`
    // returns a document beside the page, from which text and structure can be
    // taken without anything being painted or run. Libraries build their
    // sanitizers on it.
    //
    // A sibling of `createHTMLDocument`, not a second parser: same tree, same
    // arena, same frame.
    let dp_proto = new_obj(Some(realm.object_proto.clone()));
    meth(&dp_proto, "parseFromString", |i, _, a| {
        let src = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        // The second parameter is required and an enumeration; anything else is
        // a TypeError (DOM §DOMParser). Parameters after `;`
        // (`text/html;charset=utf-8`) are not part of the name.
        let ty = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        let ty = ty.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let html_kind = match &*ty {
            "text/html" => true,
            // XML goes through the same HTML parser, an approximation: a real XML
            // parse returns a `parsererror` document on any well-formedness error,
            // which this never produces.
            "text/xml" | "application/xml" | "application/xhtml+xml" | "image/svg+xml" => false,
            _ => return i.type_err(&alloc::format!(
                "parseFromString: den Typ '{ty}' gibt es in dieser Aufzaehlung nicht")),
        };
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let doc = d.parse_document(&src, html_kind);
        Ok(wrap(i, doc))
    }, 2, &fp);
    let dp_ctor = native(Some(fp.clone()), |i, _, _| {
        let proto = match i.get(&Value::Obj(i.realm.global.clone()), "DOMParser")
                          .and_then(|c| i.get(&c, "prototype")) {
            Ok(Value::Obj(o)) => o, _ => i.realm.object_proto.clone(),
        };
        Ok(Value::Obj(new_obj(Some(proto))))
    }, "DOMParser", 0, true);
    dp_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(dp_proto.clone())));
    dp_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(dp_ctor.clone())));
    dp_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("DOMParser")));
    realm.global.borrow_mut().define("DOMParser", Prop::builtin(Value::Obj(dp_ctor)));

    // A document is an `XPathEvaluator` (DOM 4 §XPathEvaluatorBase);
    // `document.evaluate(...)` is the entry pages use, `new XPathEvaluator()`
    // the one libraries use.
    meth(&document_proto, "evaluate", |i, _, a| {
        let src = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let ctx = a.get(1).cloned().unwrap_or(Value::Undefined);
        let want = match a.get(3) { Some(v) => i.to_number(v)?, None => 0.0 };
        xpath_run(i, &src, &ctx, want)
    }, 4, &fp);
    meth(&document_proto, "createExpression", |i, _, a| {
        let src = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        xpath_compiled(i, &src)?;
        let o = new_obj(Some(i.realm.xpath_expr_proto.clone()));
        o.borrow_mut().define("__xsrc", Prop { value: Some(Value::string(src)),
            get: None, set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(o))
    }, 1, &fp);
    meth(&document_proto, "createNSResolver", |_, _, a| {
        Ok(a.first().cloned().unwrap_or(Value::Null))
    }, 1, &fp);

    meth(&document_proto, "createDocumentFragment", |i, _, _| {
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, "#fragment");
        Ok(wrap(i, id))
    }, 0, &fp);

    // ── Interfaces as global constructors ────────────────────────────────
    //
    // Needed for `el instanceof HTMLLinkElement` and
    // `class X extends HTMLElement`.
    //
    // The chain is the real one: EventTarget -> Node -> Element -> HTMLElement
    // -> HTMLxyzElement. A flat list would satisfy `instanceof HTMLElement`
    // but make `link instanceof Element` false.
    let html_element_proto = new_obj(Some(element_proto.clone()));
    let svg_element_proto = new_obj(Some(element_proto.clone()));

    // `new HTMLElement()` throws, as in browsers. The class definition
    // `class X extends HTMLElement {}` still works: it only reads
    // `HTMLElement.prototype`; the constructor runs only on `new`.
    fn iface(realm: &Realm, name: &str, proto: &Gc) -> Gc {
        iface_with(realm, name, proto, |i, _, _| i.type_err("Illegal constructor"))
    }

    /// The same wiring, but with a real constructor. Most DOM interfaces have
    /// none (`new HTMLElement()` throws in browsers too); `EventTarget` has one
    /// (DOM §2.7), and pages feature-test with `new EventTarget()`.
    fn iface_with(realm: &Realm, name: &str, proto: &Gc, ctor: NativeFn) -> Gc {
        let c = native(Some(realm.function_proto.clone()), ctor, name, 0, true);
        c.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
        proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c.clone())));
        // `Symbol.toStringTag` carries the interface name (WebIDL 3.7.3).
        // Without it `Object.prototype.toString.call(el)` returns
        // `[object Object]`, and library code (jQuery's `isPlainObject`) would
        // treat every node as plain data, deep-copying into the `parentNode`
        // cycle.
        //
        // Set here because `iface` is the single way a DOM interface is created.
        proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str(name)));
        realm.global.borrow_mut().define(name, Prop::builtin(Value::Obj(c.clone())));
        c
    }
    // A standalone `EventTarget` is a detached node, so `addEventListener`,
    // `removeEventListener` and `dispatchEvent` work unchanged: the listener
    // list sits on the node, and without a parent the propagation path is one
    // node long, which is correct for a target without a tree. Nothing else
    // can reach it, neither selectors nor layout.
    iface_with(realm, "EventTarget", &event_target_proto, |i, _, _| {
        if !i.native_new { return i.type_err("Constructor EventTarget requires 'new'") }
        // The listener list lives in the document; without one there is nowhere
        // to put it. Pages always have one; only bare `jsrun` does not, and there
        // this says so instead of returning half a target.
        let Some(d) = &mut i.doc else {
            return i.type_err("EventTarget needs a document (the listener list lives there)")
        };
        let id = d.create(ELEMENT_NODE, "#eventtarget");
        Ok(wrap(i, id))
    });
    realm.event_target_proto = event_target_proto.clone();
    // The window is an EventTarget, so `window` has the same three methods as
    // every node without defining them twice.
    realm.global.borrow_mut().proto = Some(event_target_proto.clone());
    // The window is called `Window`, not `EventTarget`; otherwise it would
    // inherit its prototype's tag. Not via `iface`: that would put a
    // `constructor` on the global object, where the page's own definitions
    // live.
    realm.global.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("Window")));
    let node_ctor = iface(realm, "Node", &node_proto);
    // The node type constants, on the constructor and on the prototype as the
    // spec says. Without them `Node.ELEMENT_NODE` is `undefined`, and a
    // `switch (el.nodeType) { case Node.ELEMENT_NODE: … }` silently falls into
    // `default`.
    for (n, v) in [("ELEMENT_NODE", 1.0), ("ATTRIBUTE_NODE", 2.0), ("TEXT_NODE", 3.0),
                   ("CDATA_SECTION_NODE", 4.0), ("ENTITY_REFERENCE_NODE", 5.0),
                   ("ENTITY_NODE", 6.0), ("PROCESSING_INSTRUCTION_NODE", 7.0),
                   ("COMMENT_NODE", 8.0), ("DOCUMENT_NODE", 9.0),
                   ("DOCUMENT_TYPE_NODE", 10.0), ("DOCUMENT_FRAGMENT_NODE", 11.0),
                   ("NOTATION_NODE", 12.0)] {
        node_ctor.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
        node_proto.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    iface(realm, "Element", &element_proto);
    // After `iface`: it puts an "Illegal constructor" on the same prototype,
    // and the last write wins.
    iface(realm, "HTMLElement", &html_element_proto);
    install_dom_exception(realm);
    install_custom_elements(realm, &html_element_proto);
    iface(realm, "SVGElement", &svg_element_proto);
    // `CharacterData` sits between Node and Text; library code checks this
    // chain.
    let char_data_proto = new_obj(Some(node_proto.clone()));
    iface(realm, "CharacterData", &char_data_proto);
    text_proto.borrow_mut().proto = Some(char_data_proto.clone());
    iface(realm, "Text", &text_proto);
    // A comment is not an HTMLElement.
    let comment_proto = new_obj(Some(char_data_proto.clone()));
    iface(realm, "Comment", &comment_proto);
    accessor(&char_data_proto, "data",
        |i, t, _| with_node!(i, t, |n| Ok(Value::Str(n.text.clone()))),
        |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc { d.set_text(id, v); }
            Ok(Value::Undefined)
        }, &fp);
    getter(&char_data_proto, "length",
        |i, t, _| with_node!(i, t, |n| Ok(Value::Num(n.text.chars().count() as f64))), &fp);
    meth(&char_data_proto, "appendData", |i, t, a| {
        let id = node_of(i, &t)?;
        let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if let Some(d) = &mut i.doc {
            let mut s = d.nodes[id as usize].text.to_string();
            s.push_str(&v);
            d.set_text(id, Rc::from(s.as_str()));
        }
        Ok(Value::Undefined)
    }, 1, &fp);


    // ── Attr + NamedNodeMap ──────────────────────────────────────────────
    //
    // `el.attributes`, `NamedNodeMap.length` and the `Attr` fields depend on
    // each other: without `Attr` the map is empty, without the map
    // `attributes` is useless.
    //
    // The map is a snapshot, not a live view as in browsers: holding it while
    // setting an attribute shows the old state.
    //
    // The fields sit on the prototype, not on each object, as in browsers.
    let attr_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "Attr", &attr_proto);
    /// Read a hidden field. Builtin functions are pointers and capture
    /// nothing, so the field name is in the body.
    macro_rules! slot_getter {
        ($proto:expr, $js:literal, $slot:literal, $fallback:expr) => {
            getter($proto, $js, |i, t, _| {
                let _ = i;
                let Value::Obj(o) = &t else { return Ok($fallback) };
                Ok(o.borrow().get_own($slot).and_then(|p| p.value.clone()).unwrap_or($fallback))
            }, &fp);
        };
    }
    slot_getter!(&attr_proto, "name", "__attrname", Value::str(""));
    slot_getter!(&attr_proto, "localName", "__attrname", Value::str(""));
    slot_getter!(&attr_proto, "value", "__attrval", Value::str(""));
    // Namespaces are not tracked; `null` is the correct answer for HTML
    // attributes.
    for k in ["namespaceURI", "prefix"] {
        getter(&attr_proto, k, |_, _, _| Ok(Value::Null), &fp);
    }
    getter(&attr_proto, "specified", |_, _, _| Ok(Value::Bool(true)), &fp);
    let nnm_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "NamedNodeMap", &nnm_proto);
    slot_getter!(&nnm_proto, "length", "__len", Value::Num(0.0));
    meth(&nnm_proto, "item", |i, t, a| {
        let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
        if !(n >= 0.0) { return Ok(Value::Null) }
        Ok(match i.get(&t, &alloc::format!("{}", n as usize))? {
            Value::Undefined => Value::Null, v => v })
    }, 1, &fp);
    meth(&nnm_proto, "getNamedItem", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let len = match i.get(&t, "length")? { Value::Num(n) => n as usize, _ => 0 };
        for n in 0..len {
            let e = i.get(&t, &alloc::format!("{n}"))?;
            if matches!(i.get(&e, "name")?, Value::Str(s) if *s == *k) { return Ok(e) }
        }
        Ok(Value::Null)
    }, 1, &fp);
    realm.attr_proto = attr_proto.clone();
    realm.nnm_proto = nnm_proto.clone();
    getter(&element_proto, "attributes", |i, t, _| {
        let id = node_of(i, &t)?;
        let attrs: Vec<(Rc<str>, Rc<str>)> = i.doc.as_ref()
            .map(|d| d.nodes[id as usize].attrs.clone()).unwrap_or_default();
        let map = new_obj(Some(i.realm.nnm_proto.clone()));
        for (n, (k, v)) in attrs.iter().enumerate() {
            let a = new_obj(Some(i.realm.attr_proto.clone()));
            {
                let mut b = a.borrow_mut();
                let hide = |v: Value| Prop { value: Some(v), get: None, set: None,
                    writable: false, enumerable: false, configurable: false };
                b.define("__attrname", hide(Value::Str(k.clone())));
                b.define("__attrval", hide(Value::Str(v.clone())));
            }
            let mut m = map.borrow_mut();
            m.define(&alloc::format!("{n}"), Prop::data(Value::Obj(a.clone())));
            // Also by name: `el.attributes.href` is common and specified (WebIDL
            // `[LegacyUnenumerableNamedProperties]`).
            m.define(k, Prop::data(Value::Obj(a)));
        }
        map.borrow_mut().define("__len", Prop { value: Some(Value::Num(attrs.len() as f64)),
            get: None, set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(map))
    }, &fp);
    // `toggleAttribute(name, force?)` returns whether the attribute is present
    // afterwards; `el.setAttribute("aria-expanded", el.toggleAttribute("open"))`
    // relies on it.
    meth(&element_proto, "toggleAttribute", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let da = i.doc.as_ref().is_some_and(|d| d.nodes[id as usize].attr(&k).is_some());
        let soll = match a.get(1) {
            None | Some(Value::Undefined) => !da,
            Some(v) => v.truthy(),
        };
        if soll != da {
            if let Some(d) = &mut i.doc {
                if soll { d.set_attr_at(id, &k, ""); } else { d.remove_attr_at(id, &k); }
            }
        }
        Ok(Value::Bool(soll))
    }, 1, &fp);

    // ── Small members ────────────────────────────────────────────────────
    //
    // A missing member is a `TypeError` in foreign code, not a visible wrong
    // value.

    // `nextElementSibling`/`previousElementSibling` belong to
    // `NonDocumentTypeChildNode`, so on Element and on Text/Comment.
    for proto in [&element_proto, &char_data_proto] {
        getter(proto, "nextElementSibling",
               |i, t, _| element_sibling(i, &t, 1), &fp);
        getter(proto, "previousElementSibling",
               |i, t, _| element_sibling(i, &t, -1), &fp);
        // `remove()` belongs to `ChildNode`, so on both Element and
        // `CharacterData`.
        meth(proto, "remove", |i, t, _| {
            let id = node_of(i, &t)?;
            if let Some(d) = &mut i.doc { d.detach(id); }
            Ok(Value::Undefined)
        }, 0, &fp);
    }
    // `lastElementChild`/`childElementCount`, the siblings of
    // `firstElementChild`.
    for proto in [&element_proto, &document_proto, &fragment_proto] {
        getter(proto, "lastElementChild", |i, t, _| {
            let id = node_of(i, &t)?;
            let c = i.doc.as_ref().and_then(|d| d.nodes[id as usize].children.iter()
                .rev().copied().find(|&c| d.nodes[c as usize].kind == ELEMENT_NODE));
            Ok(match c { Some(x) => wrap(i, x), None => Value::Null })
        }, &fp);
        getter(proto, "childElementCount", |i, t, _| {
            let id = node_of(i, &t)?;
            let n = i.doc.as_ref().map_or(0, |d| d.nodes[id as usize].children.iter()
                .filter(|&&c| d.nodes[c as usize].kind == ELEMENT_NODE).count());
            Ok(Value::Num(n as f64))
        }, &fp);
        // `replaceChildren(...)`: everything out, the new nodes in.
        meth(proto, "replaceChildren", |i, t, a| {
            let id = node_of(i, &t)?;
            let alt: Vec<u32> = i.doc.as_ref()
                .map(|d| d.nodes[id as usize].children.clone()).unwrap_or_default();
            for c in alt { if let Some(d) = &mut i.doc { d.detach(c); } }
            insert_all(i, &t, a, Where::Last)
        }, 0, &fp);
    }
    // `firstElementChild`/`children` on `document` and `<html>` as well.
    for proto in [&document_proto, &fragment_proto] {
        getter(proto, "firstElementChild", |i, t, _| {
            let id = node_of(i, &t)?;
            let c = i.doc.as_ref().and_then(|d| d.nodes[id as usize].children.iter()
                .copied().find(|&c| d.nodes[c as usize].kind == ELEMENT_NODE));
            Ok(match c { Some(x) => wrap(i, x), None => Value::Null })
        }, &fp);
        getter(proto, "children", |i, t, _| {
            let id = node_of(i, &t)?;
            let cs: Vec<u32> = i.doc.as_ref().map(|d| d.nodes[id as usize].children.iter()
                .copied().filter(|&c| d.nodes[c as usize].kind == ELEMENT_NODE).collect())
                .unwrap_or_default();
            Ok(nodes_array(i, cs))
        }, &fp);
    }
    // `isConnected`: is this node attached to the document? Libraries ask
    // before measuring; a node outside the tree has no box, and measuring it
    // gives zeros that look like a measurement.
    getter(&node_proto, "isConnected", |i, t, _| {
        let id = node_of(i, &t)?;
        let Some(d) = &i.doc else { return Ok(Value::Bool(false)) };
        let mut cur = Some(id);
        while let Some(x) = cur {
            if x == d.doc { return Ok(Value::Bool(true)) }
            cur = d.nodes[x as usize].parent;
        }
        Ok(Value::Bool(false))
    }, &fp);

    // ── DOMTokenList ─────────────────────────────────────────────────────
    //
    // The methods live on one prototype, not on each list.
    let token_list_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "DOMTokenList", &token_list_proto);
    /// Write a node's classes: one place, one format.
    fn set_classes(i: &mut Interp, id: u32, cs: &[Rc<str>]) {
        let joined = cs.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
        if let Some(d) = &mut i.doc { d.set_attr_at(id, "class", &joined); }
    }
    meth(&token_list_proto, "contains", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(i.doc.as_ref().is_some_and(|d| d.classes(id).iter().any(|c| *c == k))))
    }, 1, &fp);
    meth(&token_list_proto, "add", |i, t, a| {
        let id = node_of(i, &t)?;
        for v in a {
            let k = i.to_string(v)?;
            let Some(d) = &i.doc else { break };
            let mut cs = d.classes(id);
            if cs.iter().any(|c| *c == k) { continue }
            cs.push(k);
            set_classes(i, id, &cs);
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&token_list_proto, "remove", |i, t, a| {
        let id = node_of(i, &t)?;
        for v in a {
            let k = i.to_string(v)?;
            let Some(d) = &i.doc else { break };
            let cs: Vec<Rc<str>> = d.classes(id).into_iter().filter(|c| *c != k).collect();
            set_classes(i, id, &cs);
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&token_list_proto, "toggle", |i, t, a| {
        let id = node_of(i, &t)?;
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        // `toggle(name, force)`: the second argument decides instead of the
        // current state.
        let forced = match a.get(1) { None | Some(Value::Undefined) => None, Some(v) => Some(v.truthy()) };
        let has = i.doc.as_ref().is_some_and(|d| d.classes(id).iter().any(|c| *c == k));
        let want = forced.unwrap_or(!has);
        if want != has {
            let Some(d) = &i.doc else { return Ok(Value::Bool(has)) };
            let mut cs: Vec<Rc<str>> = d.classes(id).into_iter().filter(|c| *c != k).collect();
            if want { cs.push(k); }
            set_classes(i, id, &cs);
        }
        Ok(Value::Bool(want))
    }, 1, &fp);
    meth(&token_list_proto, "replace", |i, t, a| {
        let id = node_of(i, &t)?;
        let from = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let to = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        let Some(d) = &i.doc else { return Ok(Value::Bool(false)) };
        let cs = d.classes(id);
        if !cs.iter().any(|c| *c == from) { return Ok(Value::Bool(false)) }
        let cs: Vec<Rc<str>> = cs.into_iter().map(|c| if c == from { to.clone() } else { c }).collect();
        set_classes(i, id, &cs);
        Ok(Value::Bool(true))
    }, 2, &fp);
    meth(&token_list_proto, "item", |i, t, a| {
        let id = node_of(i, &t)?;
        let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &i.doc else { return Ok(Value::Null) };
        Ok(match d.classes(id).get(n as usize) { Some(c) => Value::Str(c.clone()), None => Value::Null })
    }, 1, &fp);
    getter(&token_list_proto, "length", |i, t, _| {
        let id = node_of(i, &t)?;
        Ok(Value::Num(i.doc.as_ref().map(|d| d.classes(id).len()).unwrap_or(0) as f64))
    }, &fp);
    accessor(&token_list_proto, "value",
        |i, t, _| with_node!(i, t, |n| Ok(match n.attr("class") {
            Some(v) => Value::Str(v.clone()), None => Value::str("") })),
        |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc { d.set_attr_at(id, "class", &v); }
            Ok(Value::Undefined)
        }, &fp);
    meth(&token_list_proto, "toString", |i, t, _| {
        with_node!(i, t, |n| Ok(match n.attr("class") { Some(v) => Value::Str(v.clone()), None => Value::str("") }))
    }, 0, &fp);
    meth(&token_list_proto, "forEach", |i, t, a| {
        let id = node_of(i, &t)?;
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        let cs = i.doc.as_ref().map(|d| d.classes(id)).unwrap_or_default();
        for (k, c) in cs.into_iter().enumerate() {
            i.call(&f, Value::Undefined, &[Value::Str(c), Value::Num(k as f64), t.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    // ── CSSStyleDeclaration ──────────────────────────────────────────────
    //
    // `el.style` is a view on the `style` attribute. Assignments take effect:
    // the cascade reads the same attribute, and `dirty` tells beak to
    // relayout.
    let style_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "CSSStyleDeclaration", &style_proto);
    meth(&style_proto, "getPropertyValue", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_ascii_lowercase();
        let text = style_text(i, &t);
        Ok(match style_decls(&text).into_iter().rev().find(|(k, _)| *k == n) {
            Some((_, v)) => Value::string(v),
            None => Value::str(""),
        })
    }, 1, &fp);
    meth(&style_proto, "setProperty", |i, t, a| {
        let id = node_of(i, &t)?;
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
        style_set(i, id, &n.to_ascii_lowercase(), &v);
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&style_proto, "removeProperty", |i, t, a| {
        let id = node_of(i, &t)?;
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_ascii_lowercase();
        let old = style_get(i, id, &n);
        style_set(i, id, &n, "");
        Ok(old)
    }, 1, &fp);
    meth(&style_proto, "item", |i, t, a| {
        let n = i.to_number(a.first().unwrap_or(&Value::Undefined))? as usize;
        let text = style_text(i, &t);
        Ok(match style_decls(&text).get(n) { Some((k, _)) => Value::string(k.clone()), None => Value::str("") })
    }, 1, &fp);
    getter(&style_proto, "length", |i, t, _| {
        let text = style_text(i, &t);
        Ok(Value::Num(style_decls(&text).len() as f64))
    }, &fp);
    accessor(&style_proto, "cssText",
        |i, t, _| with_node!(i, t, |n| Ok(match n.attr("style") {
            Some(v) => Value::Str(v.clone()), None => Value::str("") })),
        |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc { d.set_attr_at(id, "style", &v); }
            Ok(Value::Undefined)
        }, &fp);
    // The named properties. The list is finite on purpose: without a Proxy
    // there is no way to intercept every name. Anything not listed goes
    // through `setProperty`/`getPropertyValue`.
    style_prop!(style_proto, fp, "display", "display");
    style_prop!(style_proto, fp, "visibility", "visibility");
    style_prop!(style_proto, fp, "opacity", "opacity");
    style_prop!(style_proto, fp, "position", "position");
    style_prop!(style_proto, fp, "top", "top");
    style_prop!(style_proto, fp, "right", "right");
    style_prop!(style_proto, fp, "bottom", "bottom");
    style_prop!(style_proto, fp, "left", "left");
    style_prop!(style_proto, fp, "zIndex", "z-index");
    style_prop!(style_proto, fp, "width", "width");
    style_prop!(style_proto, fp, "height", "height");
    style_prop!(style_proto, fp, "minWidth", "min-width");
    style_prop!(style_proto, fp, "minHeight", "min-height");
    style_prop!(style_proto, fp, "maxWidth", "max-width");
    style_prop!(style_proto, fp, "maxHeight", "max-height");
    style_prop!(style_proto, fp, "margin", "margin");
    style_prop!(style_proto, fp, "marginTop", "margin-top");
    style_prop!(style_proto, fp, "marginRight", "margin-right");
    style_prop!(style_proto, fp, "marginBottom", "margin-bottom");
    style_prop!(style_proto, fp, "marginLeft", "margin-left");
    style_prop!(style_proto, fp, "padding", "padding");
    style_prop!(style_proto, fp, "paddingTop", "padding-top");
    style_prop!(style_proto, fp, "paddingRight", "padding-right");
    style_prop!(style_proto, fp, "paddingBottom", "padding-bottom");
    style_prop!(style_proto, fp, "paddingLeft", "padding-left");
    style_prop!(style_proto, fp, "color", "color");
    style_prop!(style_proto, fp, "background", "background");
    style_prop!(style_proto, fp, "backgroundColor", "background-color");
    style_prop!(style_proto, fp, "backgroundImage", "background-image");
    style_prop!(style_proto, fp, "backgroundPosition", "background-position");
    style_prop!(style_proto, fp, "backgroundSize", "background-size");
    style_prop!(style_proto, fp, "backgroundRepeat", "background-repeat");
    style_prop!(style_proto, fp, "border", "border");
    style_prop!(style_proto, fp, "borderTop", "border-top");
    style_prop!(style_proto, fp, "borderRight", "border-right");
    style_prop!(style_proto, fp, "borderBottom", "border-bottom");
    style_prop!(style_proto, fp, "borderLeft", "border-left");
    style_prop!(style_proto, fp, "borderColor", "border-color");
    style_prop!(style_proto, fp, "borderWidth", "border-width");
    style_prop!(style_proto, fp, "borderStyle", "border-style");
    style_prop!(style_proto, fp, "borderRadius", "border-radius");
    style_prop!(style_proto, fp, "font", "font");
    style_prop!(style_proto, fp, "fontSize", "font-size");
    style_prop!(style_proto, fp, "fontFamily", "font-family");
    style_prop!(style_proto, fp, "fontWeight", "font-weight");
    style_prop!(style_proto, fp, "fontStyle", "font-style");
    style_prop!(style_proto, fp, "lineHeight", "line-height");
    style_prop!(style_proto, fp, "textAlign", "text-align");
    style_prop!(style_proto, fp, "textDecoration", "text-decoration");
    style_prop!(style_proto, fp, "textTransform", "text-transform");
    style_prop!(style_proto, fp, "letterSpacing", "letter-spacing");
    style_prop!(style_proto, fp, "whiteSpace", "white-space");
    style_prop!(style_proto, fp, "wordBreak", "word-break");
    style_prop!(style_proto, fp, "overflow", "overflow");
    style_prop!(style_proto, fp, "overflowX", "overflow-x");
    style_prop!(style_proto, fp, "overflowY", "overflow-y");
    style_prop!(style_proto, fp, "cursor", "cursor");
    style_prop!(style_proto, fp, "pointerEvents", "pointer-events");
    style_prop!(style_proto, fp, "userSelect", "user-select");
    style_prop!(style_proto, fp, "transform", "transform");
    style_prop!(style_proto, fp, "transformOrigin", "transform-origin");
    style_prop!(style_proto, fp, "transition", "transition");
    style_prop!(style_proto, fp, "animation", "animation");
    style_prop!(style_proto, fp, "filter", "filter");
    style_prop!(style_proto, fp, "boxShadow", "box-shadow");
    style_prop!(style_proto, fp, "textShadow", "text-shadow");
    style_prop!(style_proto, fp, "flex", "flex");
    style_prop!(style_proto, fp, "flexDirection", "flex-direction");
    style_prop!(style_proto, fp, "flexWrap", "flex-wrap");
    style_prop!(style_proto, fp, "flexGrow", "flex-grow");
    style_prop!(style_proto, fp, "flexShrink", "flex-shrink");
    style_prop!(style_proto, fp, "flexBasis", "flex-basis");
    style_prop!(style_proto, fp, "justifyContent", "justify-content");
    style_prop!(style_proto, fp, "alignItems", "align-items");
    style_prop!(style_proto, fp, "alignSelf", "align-self");
    style_prop!(style_proto, fp, "alignContent", "align-content");
    style_prop!(style_proto, fp, "gap", "gap");
    style_prop!(style_proto, fp, "rowGap", "row-gap");
    style_prop!(style_proto, fp, "columnGap", "column-gap");
    style_prop!(style_proto, fp, "order", "order");
    style_prop!(style_proto, fp, "gridTemplateColumns", "grid-template-columns");
    style_prop!(style_proto, fp, "gridTemplateRows", "grid-template-rows");
    style_prop!(style_proto, fp, "gridColumn", "grid-column");
    style_prop!(style_proto, fp, "gridRow", "grid-row");
    style_prop!(style_proto, fp, "clear", "clear");
    style_prop!(style_proto, fp, "content", "content");
    style_prop!(style_proto, fp, "verticalAlign", "vertical-align");
    style_prop!(style_proto, fp, "boxSizing", "box-sizing");
    style_prop!(style_proto, fp, "outline", "outline");
    style_prop!(style_proto, fp, "resize", "resize");
    style_prop!(style_proto, fp, "tableLayout", "table-layout");
    style_prop!(style_proto, fp, "listStyle", "list-style");
    style_prop!(style_proto, fp, "objectFit", "object-fit");
    style_prop!(style_proto, fp, "willChange", "will-change");
    style_prop!(style_proto, fp, "inset", "inset");
    style_prop!(style_proto, fp, "aspectRatio", "aspect-ratio");
    style_prop!(style_proto, fp, "cssFloat", "float");
    style_prop!(style_proto, fp, "float", "float");
    realm.style_proto = style_proto;
    realm.token_list_proto = token_list_proto;
    realm.comment_proto = comment_proto.clone();
    iface(realm, "Document", &document_proto);
    iface(realm, "HTMLDocument", &document_proto);
    iface(realm, "DocumentFragment", &fragment_proto);
    // `ShadowRoot` exists as an interface even without Shadow DOM, because an
    // `instanceof` against a missing name throws instead of returning false
    // (htmx checks `e.parentNode instanceof ShadowRoot`). `false` is the true
    // answer: beak never attaches a shadow tree, which is how browsers look on
    // any page that never calls `attachShadow`.
    let shadow_root_proto = new_obj(Some(fragment_proto.clone()));
    iface(realm, "ShadowRoot", &shadow_root_proto);

    // ── Event ────────────────────────────────────────────────────────────
    //
    // The fields live in slots and the prototype getters read them, so that
    // `e instanceof Event` works and `Event.prototype.target` is a real
    // accessor rather than an instance property.
    let event_proto = new_obj(Some(realm.object_proto.clone()));
    let fp2 = realm.function_proto.clone();
    // One builtin getter per field. A function pointer captures nothing, so
    // each carries its slot name in the body; the macro writes them.
    ev_getter!(event_proto, fp2, "type", EV_TYPE);
    ev_getter!(event_proto, fp2, "target", EV_TARGET);
    ev_getter!(event_proto, fp2, "srcElement", EV_TARGET);
    ev_getter!(event_proto, fp2, "currentTarget", EV_CUR);
    ev_getter!(event_proto, fp2, "bubbles", EV_BUBBLES);
    ev_getter!(event_proto, fp2, "cancelable", EV_CANCELABLE);
    ev_getter!(event_proto, fp2, "defaultPrevented", EV_PREVENTED);
    ev_getter!(event_proto, fp2, "isTrusted", EV_TRUSTED);
    ev_getter!(event_proto, fp2, "eventPhase", EV_PHASE);
    ev_getter!(event_proto, fp2, "timeStamp", EV_STAMP);
    meth(&event_proto, "preventDefault", |i, t, _| {
        // Only a cancelable event can be canceled; otherwise `defaultPrevented`
        // would report a stop nobody honours.
        if matches!(i.get(&t, EV_CANCELABLE)?, Value::Bool(true)) {
            if let Value::Obj(o) = &t { o.borrow_mut().define(EV_PREVENTED, Prop::data(Value::Bool(true))); }
        }
        Ok(Value::Undefined)
    }, 0, &fp2);
    meth(&event_proto, "stopPropagation", |_, t, _| {
        if let Value::Obj(o) = &t { o.borrow_mut().define(EV_STOP, Prop::data(Value::Bool(true))); }
        Ok(Value::Undefined)
    }, 0, &fp2);
    meth(&event_proto, "stopImmediatePropagation", |_, t, _| {
        if let Value::Obj(o) = &t {
            o.borrow_mut().define(EV_STOP, Prop::data(Value::Bool(true)));
            o.borrow_mut().define(EV_STOPIMM, Prop::data(Value::Bool(true)));
        }
        Ok(Value::Undefined)
    }, 0, &fp2);
    // `initEvent(type, bubbles, cancelable)`, the partner of `createEvent`.
    //
    // DOM §initEvent: an event being dispatched cannot be renamed (it would
    // change type mid-path), and the call resets the cancel flags so the same
    // object can be dispatched again.
    meth(&event_proto, "initEvent", |i, t, a| {
        let Value::Obj(o) = &t else { return i.type_err("initEvent: kein Ereignis") };
        // `eventPhase != NONE` is the spec's dispatch flag.
        if !matches!(i.get(&t, EV_PHASE)?, Value::Num(0.0)) {
            return Ok(Value::Undefined);
        }
        let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let bubbles = a.get(1).map(|v| v.truthy()).unwrap_or(false);
        let cancelable = a.get(2).map(|v| v.truthy()).unwrap_or(false);
        init_event_fields(o, &kind, bubbles, cancelable);
        Ok(Value::Undefined)
    }, 3, &fp2);
    meth(&event_proto, "composedPath", |i, t, _| {
        let tgt = i.get(&t, EV_TARGET)?;
        let Ok(id) = node_of(i, &tgt) else { return Ok(i.new_array(Vec::new())) };
        let chain = ancestors(i, id);
        // From the target outwards; `ancestors` returns dispatch order, outermost
        // first.
        Ok(nodes_array(i, chain.into_iter().rev().collect()))
    }, 0, &fp2);
    let event_ctor = native(Some(realm.function_proto.clone()), |i, _, a| {
        let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let init = a.get(1).cloned().unwrap_or(Value::Undefined);
        let proto = i.realm.event_proto.clone();
        let ev = build_event(i, proto, &kind, false);
        apply_event_init(i, &ev, &init)?;
        Ok(Value::Obj(ev))
    }, "Event", 1, true);
    event_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(event_proto.clone())));
    event_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(event_ctor.clone())));
    event_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("Event")));
    realm.global.borrow_mut().define("Event", Prop::builtin(Value::Obj(event_ctor)));
    for (k, v) in [("NONE", 0.0), ("CAPTURING_PHASE", 1.0), ("AT_TARGET", 2.0), ("BUBBLING_PHASE", 3.0)] {
        event_proto.borrow_mut().define(k, Prop::frozen(Value::Num(v)));
    }

    // `CustomEvent` is an `Event` with one extra field; the way pages and
    // components talk to each other.
    let custom_proto = new_obj(Some(event_proto.clone()));
    ev_getter!(custom_proto, fp2, "detail", EV_DETAIL);
    let custom_ctor = native(Some(realm.function_proto.clone()), |i, _, a| {
        let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let init = a.get(1).cloned().unwrap_or(Value::Undefined);
        let proto = match i.get(&Value::Obj(i.realm.global.clone()), "CustomEvent")
                          .and_then(|c| i.get(&c, "prototype")) {
            Ok(Value::Obj(o)) => o, _ => i.realm.event_proto.clone(),
        };
        let ev = build_event(i, proto, &kind, false);
        apply_event_init(i, &ev, &init)?;
        let detail = match &init { Value::Obj(_) => i.get(&init, "detail")?, _ => Value::Null };
        ev.borrow_mut().define(EV_DETAIL, Prop::data(detail));
        Ok(Value::Obj(ev))
    }, "CustomEvent", 1, true);
    // The partner of `createEvent("CustomEvent")`; without it the branch above
    // would return an object that never gets its `detail`.
    meth(&custom_proto, "initCustomEvent", |i, t, a| {
        let Value::Obj(o) = &t else { return i.type_err("initCustomEvent: kein Ereignis") };
        if !matches!(i.get(&t, EV_PHASE)?, Value::Num(0.0)) {
            return Ok(Value::Undefined);
        }
        let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let bubbles = a.get(1).map(|v| v.truthy()).unwrap_or(false);
        let cancelable = a.get(2).map(|v| v.truthy()).unwrap_or(false);
        init_event_fields(o, &kind, bubbles, cancelable);
        let detail = a.get(3).cloned().unwrap_or(Value::Null);
        o.borrow_mut().define(EV_DETAIL, Prop { value: Some(detail), get: None, set: None,
            writable: true, enumerable: false, configurable: true });
        Ok(Value::Undefined)
    }, 4, &fp2);
    custom_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(custom_proto.clone())));
    custom_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(custom_ctor.clone())));
    custom_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CustomEvent")));
    realm.global.borrow_mut().define("CustomEvent", Prop::builtin(Value::Obj(custom_ctor)));

    // ── UI event kinds ──────────────────────────────────────────────────
    //
    // The chain is the real one: Event -> UIEvent -> {Keyboard, Mouse, Focus,
    // Input}Event. The fields sit in slots and are read through prototype
    // accessors, as for `Event`, so they are not enumerable, as in browsers.
    let ui_proto = new_obj(Some(event_proto.clone()));
    ev_getter!(ui_proto, fp2, "detail", EV_DETAIL);
    ev_getter!(ui_proto, fp2, "view", "__evview");
    let ui_ctor = iface_with(realm, "UIEvent", &ui_proto, |i, _, a| {
        let ev = event_of_kind(i, "UIEvent", a)?;
        Ok(Value::Obj(ev))
    });
    let _ = ui_ctor;

    let kbd_proto = new_obj(Some(ui_proto.clone()));
    ev_getter!(kbd_proto, fp2, "key", "__evkey");
    ev_getter!(kbd_proto, fp2, "code", "__evcode");
    ev_getter!(kbd_proto, fp2, "keyCode", "__evkeycode");
    ev_getter!(kbd_proto, fp2, "charCode", "__evcharcode");
    ev_getter!(kbd_proto, fp2, "which", "__evkeycode");
    ev_getter!(kbd_proto, fp2, "repeat", "__evrepeat");
    ev_getter!(kbd_proto, fp2, "isComposing", "__evcomposing");
    ev_getter!(kbd_proto, fp2, "location", "__evlocation");
    ev_getter!(kbd_proto, fp2, "altKey", "__evalt");
    ev_getter!(kbd_proto, fp2, "ctrlKey", "__evctrl");
    ev_getter!(kbd_proto, fp2, "shiftKey", "__evshift");
    ev_getter!(kbd_proto, fp2, "metaKey", "__evmeta");
    // `getModifierState` reads the same four slots. Pages also ask for
    // modifiers we do not track (`CapsLock`); the correct answer there is
    // `false`, not a throw.
    meth(&kbd_proto, "getModifierState", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let slot = match &*k {
            "Alt" => "__evalt", "Control" => "__evctrl",
            "Shift" => "__evshift", "Meta" => "__evmeta",
            _ => return Ok(Value::Bool(false)),
        };
        Ok(Value::Bool(i.get(&t, slot)?.truthy()))
    }, 1, &fp2);
    iface_with(realm, "KeyboardEvent", &kbd_proto, |i, _, a| {
        let ev = event_of_kind(i, "KeyboardEvent", a)?;
        Ok(Value::Obj(ev))
    });

    let input_proto = new_obj(Some(ui_proto.clone()));
    ev_getter!(input_proto, fp2, "data", "__evdata");
    ev_getter!(input_proto, fp2, "inputType", "__evinputtype");
    ev_getter!(input_proto, fp2, "isComposing", "__evcomposing");
    iface_with(realm, "InputEvent", &input_proto, |i, _, a| {
        let ev = event_of_kind(i, "InputEvent", a)?;
        Ok(Value::Obj(ev))
    });

    let focus_proto = new_obj(Some(ui_proto.clone()));
    ev_getter!(focus_proto, fp2, "relatedTarget", "__evrelated");
    iface_with(realm, "FocusEvent", &focus_proto, |i, _, a| {
        let ev = event_of_kind(i, "FocusEvent", a)?;
        Ok(Value::Obj(ev))
    });

    let mouse_proto = new_obj(Some(ui_proto.clone()));
    ev_getter!(mouse_proto, fp2, "clientX", "__evclientx");
    ev_getter!(mouse_proto, fp2, "clientY", "__evclienty");
    ev_getter!(mouse_proto, fp2, "pageX", "__evpagex");
    ev_getter!(mouse_proto, fp2, "pageY", "__evpagey");
    ev_getter!(mouse_proto, fp2, "screenX", "__evclientx");
    ev_getter!(mouse_proto, fp2, "screenY", "__evclienty");
    ev_getter!(mouse_proto, fp2, "offsetX", "__evoffsetx");
    ev_getter!(mouse_proto, fp2, "offsetY", "__evoffsety");
    ev_getter!(mouse_proto, fp2, "button", "__evbutton");
    ev_getter!(mouse_proto, fp2, "buttons", "__evbuttons");
    ev_getter!(mouse_proto, fp2, "altKey", "__evalt");
    ev_getter!(mouse_proto, fp2, "ctrlKey", "__evctrl");
    ev_getter!(mouse_proto, fp2, "shiftKey", "__evshift");
    ev_getter!(mouse_proto, fp2, "metaKey", "__evmeta");
    ev_getter!(mouse_proto, fp2, "relatedTarget", "__evrelated");
    meth(&mouse_proto, "getModifierState", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let slot = match &*k {
            "Alt" => "__evalt", "Control" => "__evctrl",
            "Shift" => "__evshift", "Meta" => "__evmeta",
            _ => return Ok(Value::Bool(false)),
        };
        Ok(Value::Bool(i.get(&t, slot)?.truthy()))
    }, 1, &fp2);
    iface_with(realm, "MouseEvent", &mouse_proto, |i, _, a| {
        let ev = event_of_kind(i, "MouseEvent", a)?;
        Ok(Value::Obj(ev))
    });
    // `PromiseRejectionEvent`: the kind under which an unhandled rejection
    // reaches the window. beak does report unhandled rejections
    // (`promise::report_rejections` dispatches this event, then logs to the
    // console). Polyfills such as core-js check for it to decide whether
    // the native `Promise` is usable, and replace it otherwise.
    let prej_proto = new_obj(Some(event_proto.clone()));
    ev_getter!(prej_proto, fp2, "reason", EV_REASON);
    ev_getter!(prej_proto, fp2, "promise", EV_PROMISE);
    let prej_ctor = native(Some(realm.function_proto.clone()), |i, _, a| {
        let kind = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let init = a.get(1).cloned().unwrap_or(Value::Undefined);
        let proto = match i.get(&Value::Obj(i.realm.global.clone()), "PromiseRejectionEvent")
                          .and_then(|c| i.get(&c, "prototype")) {
            Ok(Value::Obj(o)) => o, _ => i.realm.event_proto.clone(),
        };
        let ev = build_event(i, proto, &kind, false);
        apply_event_init(i, &ev, &init)?;
        let (reason, promise) = match &init {
            Value::Obj(_) => (i.get(&init, "reason")?, i.get(&init, "promise")?),
            _ => (Value::Undefined, Value::Undefined),
        };
        ev.borrow_mut().define(EV_REASON, Prop::data(reason));
        ev.borrow_mut().define(EV_PROMISE, Prop::data(promise));
        Ok(Value::Obj(ev))
    }, "PromiseRejectionEvent", 2, true);
    prej_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(prej_proto.clone())));
    prej_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(prej_ctor.clone())));
    prej_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("PromiseRejectionEvent")));
    realm.global.borrow_mut().define("PromiseRejectionEvent", Prop::builtin(Value::Obj(prej_ctor)));
    realm.prej_proto = prej_proto;
    realm.event_proto = event_proto;

    // The window's scroll position: accessors on the geometry, the same source
    // `getBoundingClientRect` uses.
    for k in ["scrollX", "pageXOffset"] {
        getter(&realm.global, k, |i, _, _| {
            Ok(Value::Num(i.geometry.as_ref().map_or(0.0, |g| g.scroll.0 as f64)))
        }, &fp);
    }
    for k in ["scrollY", "pageYOffset"] {
        getter(&realm.global, k, |i, _, _| {
            Ok(Value::Num(i.geometry.as_ref().map_or(0.0, |g| g.scroll.1 as f64)))
        }, &fp);
    }
    def_global(realm, "scrollTo", |i, _, a| {
        let (x, y) = scroll_args(i, a)?;
        i.want_scroll(x, y);
        Ok(Value::Undefined)
    }, 2, &fp);
    def_global(realm, "scroll", |i, _, a| {
        let (x, y) = scroll_args(i, a)?;
        i.want_scroll(x, y);
        Ok(Value::Undefined)
    }, 2, &fp);
    def_global(realm, "scrollBy", |i, _, a| {
        let (dx, dy) = scroll_args(i, a)?;
        let (sx, sy) = i.geometry.as_ref().map_or((0.0, 0.0),
            |g| (g.scroll.0 as f64, g.scroll.1 as f64));
        i.want_scroll(dx.map(|v| sx + v), dy.map(|v| sy + v));
        Ok(Value::Undefined)
    }, 2, &fp);
    def_global(realm, "getComputedStyle", |i, _, a| {
        let id = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let g = new_obj(Some(i.realm.style_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        // The computed style, if the host provided a cascade context: a
        // snapshot, which is what `getComputedStyle` returns in browsers too.
        // Values come out in CSSOM serialization (`rgb(0, 0, 0)`, not `#000`).
        // Without a context it falls back to the inline style, a partial answer
        // that keeps the page running.
        //
        // Computed on the live tree; the id is the arena index, the same number
        // the JS object holds. An element not attached anywhere has no computed
        // style, and the empty answer is the honest one.
        if let Some(text) = computed_decls(i, id) {
            g.borrow_mut().define(COMPUTED, Prop { value: Some(Value::str(&text)), get: None,
                set: None, writable: false, enumerable: false, configurable: false });
        }
        Ok(Value::Obj(g))
    }, 1, &fp);

    // ── Handlers as properties ───────────────────────────────────────────
    //
    // `el.onclick = f`. Only events in `DISPATCHED` are actually dispatched
    // by this path; the other names are accepted because the assignment must
    // not throw, and in browsers it does nothing until the event happens.
    handler_prop!(html_element_proto, fp, "onclick", "click");
    handler_prop!(html_element_proto, fp, "onload", "load");
    handler_prop!(html_element_proto, fp, "onerror", "error");
    handler_prop!(html_element_proto, fp, "onchange", "change");
    handler_prop!(html_element_proto, fp, "oninput", "input");
    handler_prop!(html_element_proto, fp, "onsubmit", "submit");
    // `focus()` / `blur()`: the page decides where the keyboard writes.
    //
    // Focus lives in the document: `document.activeElement` reads the same
    // place, and the host adopts it into its own (`forms.rs`). The events are
    // really dispatched; a `focus()` that only set a value would be invisible
    // to a page with `onfocus`.
    meth(&html_element_proto, "focus", |i, t, _| {
        let id = node_of(i, &t)?;
        let old = i.doc.as_ref().and_then(|d| d.focused);
        if old == Some(id) { return Ok(Value::Undefined) }
        if let Some(o) = old { deliver_focus(i, o, "blur")?; }
        if let Some(d) = &mut i.doc { d.focused = Some(id); d.touch(); }
        deliver_focus(i, id, "focus")?;
        Ok(Value::Undefined)
    }, 0, &fp);
    meth(&html_element_proto, "blur", |i, t, _| {
        let id = node_of(i, &t)?;
        if i.doc.as_ref().and_then(|d| d.focused) != Some(id) { return Ok(Value::Undefined) }
        if let Some(d) = &mut i.doc { d.focused = None; d.touch(); }
        deliver_focus(i, id, "blur")?;
        Ok(Value::Undefined)
    }, 0, &fp);
    handler_prop!(html_element_proto, fp, "onfocus", "focus");
    handler_prop!(html_element_proto, fp, "onblur", "blur");
    handler_prop!(html_element_proto, fp, "onkeydown", "keydown");
    handler_prop!(html_element_proto, fp, "onkeyup", "keyup");
    handler_prop!(html_element_proto, fp, "onkeypress", "keypress");
    handler_prop!(html_element_proto, fp, "onmousedown", "mousedown");
    handler_prop!(html_element_proto, fp, "onmouseup", "mouseup");
    handler_prop!(html_element_proto, fp, "onmouseover", "mouseover");
    handler_prop!(html_element_proto, fp, "onmouseout", "mouseout");
    handler_prop!(html_element_proto, fp, "onmousemove", "mousemove");
    handler_prop!(html_element_proto, fp, "onscroll", "scroll");
    handler_prop!(html_element_proto, fp, "onresize", "resize");
    handler_prop!(html_element_proto, fp, "oncontextmenu", "contextmenu");
    handler_prop!(html_element_proto, fp, "ondblclick", "dblclick");
    // Pointer events. None are dispatched yet, but the assignment must not
    // throw: in browsers `el.onpointermove = f` does nothing while the pointer
    // stays still.
    handler_prop!(html_element_proto, fp, "onpointerenter", "pointerenter");
    handler_prop!(html_element_proto, fp, "onpointerleave", "pointerleave");
    handler_prop!(html_element_proto, fp, "onpointermove", "pointermove");
    handler_prop!(html_element_proto, fp, "onpointerdown", "pointerdown");
    handler_prop!(html_element_proto, fp, "onpointerup", "pointerup");
    handler_prop!(html_element_proto, fp, "ontouchstart", "touchstart");
    handler_prop!(html_element_proto, fp, "ontouchend", "touchend");
    handler_prop!(html_element_proto, fp, "onmessage", "message");
    handler_prop!(html_element_proto, fp, "onbeforeunload", "beforeunload");
    handler_prop!(html_element_proto, fp, "onunload", "unload");
    handler_prop!(html_element_proto, fp, "ondomcontentloaded", "DOMContentLoaded");

    // The same on the window: `window.onload = …` is the oldest form of all.
    handler_prop!(realm.global, fp, "onclick", "click");
    handler_prop!(realm.global, fp, "onload", "load");
    handler_prop!(realm.global, fp, "onerror", "error");
    handler_prop!(realm.global, fp, "onchange", "change");
    handler_prop!(realm.global, fp, "oninput", "input");
    handler_prop!(realm.global, fp, "onsubmit", "submit");
    handler_prop!(realm.global, fp, "onfocus", "focus");
    handler_prop!(realm.global, fp, "onblur", "blur");
    handler_prop!(realm.global, fp, "onkeydown", "keydown");
    handler_prop!(realm.global, fp, "onkeyup", "keyup");
    handler_prop!(realm.global, fp, "onkeypress", "keypress");
    handler_prop!(realm.global, fp, "onmousedown", "mousedown");
    handler_prop!(realm.global, fp, "onmouseup", "mouseup");
    handler_prop!(realm.global, fp, "onmouseover", "mouseover");
    handler_prop!(realm.global, fp, "onmouseout", "mouseout");
    handler_prop!(realm.global, fp, "onmousemove", "mousemove");
    handler_prop!(realm.global, fp, "onscroll", "scroll");
    handler_prop!(realm.global, fp, "onresize", "resize");
    handler_prop!(realm.global, fp, "oncontextmenu", "contextmenu");
    handler_prop!(realm.global, fp, "ondblclick", "dblclick");
    handler_prop!(realm.global, fp, "ontouchstart", "touchstart");
    handler_prop!(realm.global, fp, "ontouchend", "touchend");
    handler_prop!(realm.global, fp, "onmessage", "message");
    handler_prop!(realm.global, fp, "onbeforeunload", "beforeunload");
    handler_prop!(realm.global, fp, "onunload", "unload");
    handler_prop!(realm.global, fp, "ondomcontentloaded", "DOMContentLoaded");

    // ── Fields backed by an attribute ────────────────────────────────────
    attr_prop!(html_element_proto, fp, "title", "title");
    attr_prop!(html_element_proto, fp, "lang", "lang");
    attr_prop!(html_element_proto, fp, "dir", "dir");
    attr_prop!(html_element_proto, fp, "accessKey", "accesskey");
    attr_prop!(html_element_proto, fp, "contentEditable", "contenteditable");
    bool_attr_prop!(html_element_proto, fp, "hidden", "hidden");
    // `tabIndex` is -1 when nothing is set: not reachable with the Tab key.
    // 0 would mean the opposite.
    num_attr_prop!(html_element_proto, fp, "tabIndex", "tabindex", -1.0);
    attr_prop!(element_proto, fp, "slot", "slot");
    // ── ARIA reflection ──────────────────────────────────────────────────
    //
    // `role` is the exception in the list: the attribute has the same name.
    attr_prop!(element_proto, fp, "role", "role");
    attr_prop_null!(element_proto, fp, "ariaHidden", "aria-hidden");
    attr_prop_null!(element_proto, fp, "ariaLabel", "aria-label");
    attr_prop_null!(element_proto, fp, "ariaExpanded", "aria-expanded");
    attr_prop_null!(element_proto, fp, "ariaChecked", "aria-checked");
    attr_prop_null!(element_proto, fp, "ariaSelected", "aria-selected");
    attr_prop_null!(element_proto, fp, "ariaDisabled", "aria-disabled");
    attr_prop_null!(element_proto, fp, "ariaCurrent", "aria-current");
    attr_prop_null!(element_proto, fp, "ariaPressed", "aria-pressed");
    attr_prop_null!(element_proto, fp, "ariaLive", "aria-live");
    attr_prop_null!(element_proto, fp, "ariaValueNow", "aria-valuenow");

    let mut tag_protos: HashMap<&'static str, Gc> = HashMap::new();
    for (iname, tags) in HTML_IFACES {
        let proto = new_obj(Some(html_element_proto.clone()));
        iface(realm, iname, &proto);
        for t in *tags { tag_protos.insert(t, proto.clone()); }
    }
    // Per interface, the members pages actually query.
    //
    // `href` and `src` are raw, as in the attribute. In browsers they are
    // resolved (`a.href` of a relative URL is absolute); not implemented, so
    // `.href` equals `getAttribute("href")`.
    // HTMLAnchorElement
    if let Some(p) = tag_protos.get("a") {
        attr_prop!(p, fp, "href", "href");
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "target", "target");
        attr_prop!(p, fp, "rel", "rel");
        attr_prop!(p, fp, "download", "download");
        attr_prop!(p, fp, "hreflang", "hreflang");
        // `a.hash`: derived from the raw `href`, like its neighbours.
        getter(p, "hash", |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr("href").and_then(|h| h.find('#').map(|k| h[k..].to_string())) {
                Some(f) => Value::string(f), None => Value::str("") }))
        }, &fp);
    }
    // HTMLLinkElement
    if let Some(p) = tag_protos.get("link") {
        attr_prop!(p, fp, "href", "href");
        attr_prop!(p, fp, "rel", "rel");
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "as", "as");
        attr_prop!(p, fp, "media", "media");
    }
    // HTMLScriptElement
    if let Some(p) = tag_protos.get("script") {
        attr_prop!(p, fp, "src", "src");
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "charset", "charset");
    }
    // HTMLImageElement
    if let Some(p) = tag_protos.get("img") {
        attr_prop!(p, fp, "src", "src");
        attr_prop!(p, fp, "alt", "alt");
        attr_prop!(p, fp, "srcset", "srcset");
        attr_prop!(p, fp, "sizes", "sizes");
        attr_prop!(p, fp, "loading", "loading");
        // `currentSrc`: the source actually used. beak picks from `srcset` in
        // layout (`picture.rs`), which the engine does not see, so this returns
        // `src`. Correct for images without `srcset`; for those with one it is
        // the source in the markup.
        getter(p, "currentSrc", |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr("src") {
                Some(v) => Value::Str(v.clone()), None => Value::str("") }))
        }, &fp);
    }
    // `el.click()`.
    //
    // The order is the spec's (HTML §4.10.5, synthetic click activation): a
    // checkbox or radio button toggles first, then `click` is dispatched, and
    // only if nobody canceled do `input` and `change` follow. If canceled,
    // the toggle is undone; that is the difference between `preventDefault()`
    // on a checkbox and on a button.
    meth(&html_element_proto, "click", |i, t, _| {
        let id = node_of(i, &t)?;
        let (tag, kind) = {
            let d = i.doc.as_ref();
            let n = d.and_then(|d| d.nodes.get(id as usize));
            (n.map(|n| n.tag.to_string()).unwrap_or_default(),
             n.and_then(|n| n.attr("type").map(|t| t.to_ascii_lowercase())).unwrap_or_default())
        };
        let toggles = tag == "input" && matches!(kind.as_str(), "checkbox" | "radio");
        let before = checked_now(i, id);
        if toggles {
            set_checked(i, id, if kind == "radio" { true } else { !before });
        }
        let chain = ancestors(i, id);
        let prevented = dispatch(i, "click", &chain)?;
        if toggles {
            if prevented {
                set_checked(i, id, before);
            } else {
                dispatch(i, "input", &chain)?;
                dispatch(i, "change", &chain)?;
            }
        }
        Ok(Value::Undefined)
    }, 0, &fp);

    // HTMLCanvasElement
    if let Some(p) = tag_protos.get("canvas") {
        // `width`/`height` are numbers backed by attributes, with the spec
        // defaults 300x150.
        accessor(p, "width",
            |i, t, _| with_node!(i, t, |n| Ok(Value::Num(
                n.attr("width").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(300.0)))),
            |i, t, a| {
                let id = node_of(i, &t)?;
                let v = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
                if let Some(d) = &mut i.doc { d.set_attr_at(id, "width", &alloc::format!("{}", v as i64)); }
                Ok(Value::Undefined)
            }, &fp);
        accessor(p, "height",
            |i, t, _| with_node!(i, t, |n| Ok(Value::Num(
                n.attr("height").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(150.0)))),
            |i, t, a| {
                let id = node_of(i, &t)?;
                let v = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
                if let Some(d) = &mut i.doc { d.set_attr_at(id, "height", &alloc::format!("{}", v as i64)); }
                Ok(Value::Undefined)
            }, &fp);
        // `getContext` returns `null`, which is what the spec prescribes for a
        // context type the implementation does not offer; beak has no 2D context.
        // Omitting the method would throw and end the script, while `null` lets
        // the usual `if (ctx)` guard work. Not implemented: a real 2D context.
        meth(p, "getContext", |_, _, _| Ok(Value::Null), 1, &fp);
    }
    // HTMLInputElement
    if let Some(p) = tag_protos.get("input") {
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "name", "name");
        attr_prop!(p, fp, "placeholder", "placeholder");
    }
    // ── The value of a control ───────────────────────────────────────────
    //
    // `value` is the dirty value, `defaultValue` the attribute; the spec keeps
    // them separate, and setting `.value` does not change the attribute.
    for (tag, from_text) in [("input", false), ("textarea", true)] {
        let Some(p) = tag_protos.get(tag) else { continue };
        if from_text {
            accessor(p, "value",
                |i, t, _| { let id = node_of(i, &t)?; Ok(Value::string(control_value(i, id, true))) },
                |i, t, a| { let id = node_of(i, &t)?;
                            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                            set_control_value(i, id, &v); Ok(Value::Undefined) }, &fp);
            accessor(p, "defaultValue",
                |i, t, _| { let id = node_of(i, &t)?;
                            let s = i.doc.as_ref().map(|d| d.text_of(id)).unwrap_or_default();
                            Ok(Value::string(s)) },
                |i, t, a| { let id = node_of(i, &t)?;
                            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                            set_text_of(i, id, &v); Ok(Value::Undefined) }, &fp);
        } else {
            accessor(p, "value",
                |i, t, _| { let id = node_of(i, &t)?; Ok(Value::string(control_value(i, id, false))) },
                |i, t, a| { let id = node_of(i, &t)?;
                            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                            set_control_value(i, id, &v); Ok(Value::Undefined) }, &fp);
            attr_prop!(p, fp, "defaultValue", "value");
            accessor(p, "checked",
                |i, t, _| { let id = node_of(i, &t)?;
                            let d = i.doc.as_ref();
                            Ok(Value::Bool(match d.and_then(|d| d.nodes[id as usize].checked) {
                                Some(b) => b,
                                None => d.is_some_and(|d| d.nodes[id as usize].attr("checked").is_some()),
                            })) },
                |i, t, a| { let id = node_of(i, &t)?;
                            let on = a.first().map(|v| v.truthy()).unwrap_or(false);
                            if let Some(d) = &mut i.doc { d.nodes[id as usize].checked = Some(on); d.touch(); }
                            Ok(Value::Undefined) }, &fp);
            bool_attr_prop!(p, fp, "defaultChecked", "checked");
            // `indeterminate` is not an attribute; it lives only on the object (HTML
            // §4.10.5.3). beak does not paint the third state, but scripts set it and
            // read it back.
            accessor(p, "indeterminate",
                |i, t, _| Ok(Value::Bool(matches!(i.get(&t, "__indet")?, Value::Bool(true)))),
                |i, t, a| {
                    let on = a.first().map(|v| v.truthy()).unwrap_or(false);
                    if let Value::Obj(o) = &t {
                        o.borrow_mut().define("__indet", Prop { value: Some(Value::Bool(on)),
                            get: None, set: None, writable: true, enumerable: false, configurable: true });
                    }
                    Ok(Value::Undefined)
                }, &fp);
        }
    }

    // The properties every control has: `disabled`, `readOnly`, `required`,
    // plus `form`/`labels`, the way from a field to its context.
    for tag in ["input", "select", "textarea", "button"] {
        let Some(p) = tag_protos.get(tag) else { continue };
        bool_attr_prop!(p, fp, "disabled", "disabled");
        bool_attr_prop!(p, fp, "required", "required");
        bool_attr_prop!(p, fp, "readOnly", "readonly");
        getter(p, "form", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(match owning_form(i, id) { Some(f) => wrap(i, f), None => Value::Null })
        }, &fp);
        getter(p, "labels", |i, t, _| {
            let id = node_of(i, &t)?;
            let ls = labels_of(i, id);
            Ok(nodes_array(i, ls))
        }, &fp);
    }
    // `<label>`: the two properties that lead from the label to its control.
    if let Some(p) = tag_protos.get("label") {
        attr_prop!(p, fp, "htmlFor", "for");
        getter(p, "control", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(match label_control(i, id) { Some(c) => wrap(i, c), None => Value::Null })
        }, &fp);
        getter(p, "form", |i, t, _| {
            let id = node_of(i, &t)?;
            let c = label_control(i, id).unwrap_or(id);
            Ok(match owning_form(i, c) { Some(f) => wrap(i, f), None => Value::Null })
        }, &fp);
    }
    // Text selection in a field. beak tracks no selection, so `textLength`
    // measures for real, `select`/`setSelectionRange` set focus and report no
    // selection that does not exist.
    for tag in ["input", "textarea"] {
        let Some(p) = tag_protos.get(tag) else { continue };
        getter(p, "textLength", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(Value::Num(control_value(i, id, t_is_textarea(i, id)).chars().count() as f64))
        }, &fp);
        meth(p, "select", |_, _, _| Ok(Value::Undefined), 0, &fp);
        meth(p, "setSelectionRange", |_, _, _| Ok(Value::Undefined), 2, &fp);
    }

    // ── HTMLFormElement: elements, submit, reset ─────────────────────────
    if let Some(p) = tag_protos.get("form") {
        attr_prop!(p, fp, "name", "name");
        attr_prop!(p, fp, "enctype", "enctype");
        getter(p, "elements", |i, t, _| {
            let id = node_of(i, &t)?;
            let cs = form_controls(i, id);
            Ok(nodes_array(i, cs))
        }, &fp);
        getter(p, "length", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(Value::Num(form_controls(i, id).len() as f64))
        }, &fp);
        // `submit()` submits without a `submit` event; that is the difference to
        // `requestSubmit()`, and pages rely on their own `onsubmit` not running
        // again.
        meth(p, "submit", |i, t, _| {
            let id = node_of(i, &t)?;
            let seq = i.doc.as_ref().map(|d| d.nodes[id as usize].seq).unwrap_or(0);
            if seq != 0 { i.submits.push(seq); }
            Ok(Value::Undefined)
        }, 0, &fp);
        meth(p, "requestSubmit", |i, t, _| {
            let id = node_of(i, &t)?;
            if dispatch(i, "submit", &[id])? { return Ok(Value::Undefined) }
            let seq = i.doc.as_ref().map(|d| d.nodes[id as usize].seq).unwrap_or(0);
            if seq != 0 { i.submits.push(seq); }
            Ok(Value::Undefined)
        }, 0, &fp);
        meth(p, "reset", |i, t, _| {
            let id = node_of(i, &t)?;
            for c in form_controls(i, id) {
                if let Some(d) = &mut i.doc {
                    d.nodes[c as usize].value = None;
                    d.nodes[c as usize].checked = None;
                }
            }
            if let Some(d) = &mut i.doc { d.touch(); }
            dispatch(i, "reset", &[id])?;
            Ok(Value::Undefined)
        }, 0, &fp);
    }

    // ── HTMLSelectElement / HTMLOptionElement ────────────────────────────
    //
    // The tree is the truth, not side state: `selected` is the attribute, and
    // `value` falls back to the text. Layout reads the selection the same way
    // (`forms.rs::collect_options`), so the two cannot diverge.
    if let Some(p) = tag_protos.get("option") {
        attr_prop!(p, fp, "label", "label");
        bool_attr_prop!(p, fp, "selected", "selected");
        bool_attr_prop!(p, fp, "defaultSelected", "selected");
        bool_attr_prop!(p, fp, "disabled", "disabled");
        accessor(p, "value", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(Value::string(option_value(i, id)))
        }, |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            if let Some(d) = &mut i.doc { d.set_attr_at(id, "value", &v); }
            Ok(Value::Undefined)
        }, &fp);
        accessor(p, "text", |i, t, _| {
            let id = node_of(i, &t)?;
            let s = i.doc.as_ref().map(|d| d.text_of(id)).unwrap_or_default();
            Ok(Value::string(s.trim().to_string()))
        }, |i, t, a| {
            let id = node_of(i, &t)?;
            let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            set_text_of(i, id, &v);
            Ok(Value::Undefined)
        }, &fp);
        getter(p, "index", |i, t, _| {
            let id = node_of(i, &t)?;
            let Some(sel) = owning_select(i, id) else { return Ok(Value::Num(0.0)) };
            let opts = select_options(i, sel);
            Ok(Value::Num(opts.iter().position(|x| *x == id).map(|n| n as f64).unwrap_or(-1.0)))
        }, &fp);
    }
    if let Some(p) = tag_protos.get("select") {
        attr_prop!(p, fp, "name", "name");
        bool_attr_prop!(p, fp, "multiple", "multiple");
        bool_attr_prop!(p, fp, "disabled", "disabled");
        getter(p, "options", |i, t, _| {
            let id = node_of(i, &t)?;
            let opts = select_options(i, id);
            Ok(nodes_array(i, opts))
        }, &fp);
        getter(p, "length", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(Value::Num(select_options(i, id).len() as f64))
        }, &fp);
        getter(p, "selectedOptions", |i, t, _| {
            let id = node_of(i, &t)?;
            let opts: Vec<u32> = select_options(i, id).into_iter()
                .filter(|o| i.doc.as_ref().is_some_and(|d| d.nodes[*o as usize].attr("selected").is_some()))
                .collect();
            Ok(nodes_array(i, opts))
        }, &fp);
        accessor(p, "selectedIndex", |i, t, _| {
            let id = node_of(i, &t)?;
            Ok(Value::Num(selected_index(i, id)))
        }, |i, t, a| {
            let id = node_of(i, &t)?;
            let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
            select_index(i, id, n as i64);
            Ok(Value::Undefined)
        }, &fp);
        accessor(p, "value", |i, t, _| {
            let id = node_of(i, &t)?;
            let idx = selected_index(i, id);
            if idx < 0.0 { return Ok(Value::str("")); }
            let opts = select_options(i, id);
            match opts.get(idx as usize) {
                Some(o) => { let o = *o; Ok(Value::string(option_value(i, o))) }
                None => Ok(Value::str("")),
            }
        }, |i, t, a| {
            let id = node_of(i, &t)?;
            let want = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let opts = select_options(i, id);
            let mut hit = -1i64;
            for (n, o) in opts.iter().enumerate() {
                if option_value(i, *o) == *want { hit = n as i64; break }
            }
            select_index(i, id, hit);
            Ok(Value::Undefined)
        }, &fp);
    }
    // HTMLButtonElement
    if let Some(p) = tag_protos.get("button") {
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "name", "name");
    }
    // HTMLFormElement
    if let Some(p) = tag_protos.get("form") {
        attr_prop!(p, fp, "action", "action");
        attr_prop!(p, fp, "method", "method");
        attr_prop!(p, fp, "target", "target");
    }
    // HTMLTextAreaElement
    if let Some(p) = tag_protos.get("textarea") {
        attr_prop!(p, fp, "name", "name");
        attr_prop!(p, fp, "placeholder", "placeholder");
    }
    // HTMLIFrameElement
    if let Some(p) = tag_protos.get("iframe") {
        attr_prop!(p, fp, "src", "src");
        attr_prop!(p, fp, "srcdoc", "srcdoc");
    }
    // HTMLSourceElement
    if let Some(p) = tag_protos.get("source") {
        attr_prop!(p, fp, "src", "src");
        attr_prop!(p, fp, "srcset", "srcset");
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "media", "media");
    }
    // HTMLMetaElement
    if let Some(p) = tag_protos.get("meta") {
        attr_prop!(p, fp, "name", "name");
        attr_prop!(p, fp, "content", "content");
        attr_prop!(p, fp, "charset", "charset");
    }

    // `<template>.content`. Per spec a template's content is not in the tree
    // but in its own fragment.
    //
    // The children move there on first access. Without a `.content` read they
    // stay in the tree and `to_dom` writes them back; they are not painted
    // anyway (`style.rs` gives `<template>` no box).
    if let Some(tpl) = tag_protos.get("template") {
        getter(tpl, "content", |i, t, _| {
            let id = node_of(i, &t)?;
            if let Some(f) = i.doc.as_ref().and_then(|d| d.nodes[id as usize].content) {
                return Ok(wrap(i, f));
            }
            let Some(d) = &mut i.doc else { return i.type_err("no document") };
            let f = d.create(ELEMENT_NODE, "#fragment");
            // Template contents belong to an inert document: nothing in them is
            // upgraded.
            d.nodes[f as usize].ce = CE_INERT;
            for k in d.nodes[id as usize].children.clone() { d.append(f, k); }
            d.nodes[id as usize].content = Some(f);
            Ok(wrap(i, f))
        }, &fp);
    }
    // SVG has exactly one distinction pages actually query.
    {
        let p = new_obj(Some(svg_element_proto.clone()));
        iface(realm, "SVGSVGElement", &p);
        tag_protos.insert("svg", p);
    }

    install_text_codec(realm);
    install_xpath(realm);
    install_formdata(realm);

    realm.node_proto = node_proto;
    realm.element_proto = element_proto;
    realm.text_proto = text_proto;
    realm.document_proto = document_proto;
    realm.html_element_proto = html_element_proto;
    realm.svg_element_proto = svg_element_proto;
    realm.fragment_proto = fragment_proto;
    realm.tag_protos = tag_protos;
}

/// Which element carries which interface.
///
/// Not complete by design: it covers what pages query. Anything else is
/// `HTMLElement`, which is correct for `instanceof HTMLElement`;
/// `HTMLUnknownElement` would be more precise and of no practical use.
const HTML_IFACES: &[(&str, &[&str])] = &[
    ("HTMLAnchorElement",    &["a"]),
    ("HTMLLinkElement",      &["link"]),
    ("HTMLScriptElement",    &["script"]),
    ("HTMLStyleElement",     &["style"]),
    ("HTMLImageElement",     &["img"]),
    ("HTMLInputElement",     &["input"]),
    ("HTMLButtonElement",    &["button"]),
    ("HTMLFormElement",      &["form"]),
    ("HTMLSelectElement",    &["select"]),
    ("HTMLOptionElement",    &["option"]),
    ("HTMLTextAreaElement",  &["textarea"]),
    ("HTMLLabelElement",     &["label"]),
    ("HTMLDivElement",       &["div"]),
    ("HTMLSpanElement",      &["span"]),
    ("HTMLParagraphElement", &["p"]),
    ("HTMLUListElement",     &["ul"]),
    ("HTMLOListElement",     &["ol"]),
    ("HTMLLIElement",        &["li"]),
    ("HTMLTableElement",     &["table"]),
    ("HTMLTableRowElement",  &["tr"]),
    ("HTMLTableCellElement", &["td", "th"]),
    ("HTMLHeadingElement",   &["h1", "h2", "h3", "h4", "h5", "h6"]),
    ("HTMLHtmlElement",      &["html"]),
    ("HTMLHeadElement",      &["head"]),
    ("HTMLBodyElement",      &["body"]),
    ("HTMLMetaElement",      &["meta"]),
    ("HTMLTitleElement",     &["title"]),
    ("HTMLCanvasElement",    &["canvas"]),
    ("HTMLVideoElement",     &["video"]),
    ("HTMLAudioElement",     &["audio"]),
    ("HTMLIFrameElement",    &["iframe"]),
    ("HTMLTemplateElement",  &["template"]),
    ("HTMLPictureElement",   &["picture"]),
    ("HTMLSourceElement",    &["source"]),
    ("HTMLBRElement",        &["br"]),
    ("HTMLHRElement",        &["hr"]),
    ("HTMLPreElement",       &["pre"]),
    ("HTMLDetailsElement",   &["details"]),
    ("HTMLDialogElement",    &["dialog"]),
];

/// The next/previous element sibling. Like `sibling`, but keeps going
/// until an element comes; text nodes between two `<li>` are the norm.
fn element_sibling(i: &mut Interp, this: &Value, dir: i32) -> C<Value> {
    let id = node_of(i, this)?;
    let Some(d) = &i.doc else { return Ok(Value::Null) };
    let Some(p) = d.nodes[id as usize].parent else { return Ok(Value::Null) };
    let cs = &d.nodes[p as usize].children;
    let Some(pos) = cs.iter().position(|&c| c == id) else { return Ok(Value::Null) };
    let found = if dir > 0 {
        cs[pos + 1..].iter().copied().find(|&c| d.nodes[c as usize].kind == ELEMENT_NODE)
    } else {
        cs[..pos].iter().rev().copied().find(|&c| d.nodes[c as usize].kind == ELEMENT_NODE)
    };
    Ok(match found { Some(x) => wrap(i, x), None => Value::Null })
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


// ── Event dispatch ──────────────────────────────────────────────────────────

/// The events beak dispatches through the layout hit path.
///
/// Kept in line with what the host actually fires. Adding `onload` here
/// would force hit boxes onto every page for an event that never comes
/// this way.
pub const DISPATCHED: &[&str] = &["click"];

/// Is `k` an attribute handler for one of these?
fn is_handler_attr(k: &str) -> bool {
    k.len() > 2 && k.starts_with("on") && DISPATCHED.contains(&&k[2..])
}

/// Compile the handler from `on<type>`, if there is one.
///
/// An attribute is source text, compiled only when fired: most of these
/// handlers never fire, so compiling them at load would be wasted.
///
/// An attribute that fails to compile is dropped silently, as browsers do;
/// a typo in one attribute must not stop dispatch.
fn inline_handler(i: &mut Interp, node: u32, kind: &str) -> C<Option<Value>> {
    let mut name = String::from("on");
    name.push_str(kind);
    let src = match &i.doc {
        Some(d) => match d.nodes[node as usize].attr(&name) { Some(v) => v.to_string(), None => return Ok(None) },
        None => return Ok(None),
    };
    if src.trim().is_empty() { return Ok(None); }
    // The body runs with `event` as a name and `this` bound to the element,
    // which is how attribute handlers are defined.
    let mut wrapped = String::from("(function(event){");
    wrapped.push_str(&src);
    wrapped.push_str("\n})");
    let prog = match super::parse(&wrapped, false) { Ok(p) => p, Err(_) => return Ok(None) };
    match i.run_program(&prog) {
        Ok(v) if i.is_callable(&v) => Ok(Some(v)),
        _ => Ok(None),
    }
}

/// Dispatch an event beak itself fires along a propagation path.
///
/// `chain` comes from layout, outermost first, not from the tree: the hit
/// point only knows boxes. Dispatch runs in reverse, from the target
/// outwards, as bubbling does.
pub fn dispatch(i: &mut Interp, kind: &str, chain: &[u32]) -> C<bool> {
    dispatch_at(i, kind, chain, None)
}

/// Like `dispatch`, but with the pointer position, which makes it a
/// `MouseEvent`. `client*` is viewport-relative, `page*` document-relative;
/// the caller passes both because only it knows the scroll position.
pub fn dispatch_at(i: &mut Interp, kind: &str, chain: &[u32],
                   at: Option<(f64, f64, f64, f64)>) -> C<bool> {
    if chain.is_empty() { return Ok(false); }
    let proto = match at {
        Some(_) => iface_proto(i, "MouseEvent"),
        None => i.realm.event_proto.clone(),
    };
    let ev = build_event(i, proto, kind, true);
    {
        let mut o = ev.borrow_mut();
        let mut put = |k: &str, v: Value| {
            o.define(k, Prop { value: Some(v), get: None, set: None,
                writable: true, enumerable: false, configurable: true });
        };
        put(EV_BUBBLES, Value::Bool(true));
        if let Some((cx, cy, px, py)) = at {
            for (k, v) in [("__evclientx", cx), ("__evclienty", cy),
                           ("__evpagex", px), ("__evpagey", py),
                           ("__evoffsetx", 0.0), ("__evoffsety", 0.0)] {
                put(k, Value::Num(v));
            }
            // The primary button: `button` counts from zero, `buttons` is a bitmask
            // and already empty during `click`.
            put("__evbutton", Value::Num(0.0));
            put("__evbuttons", Value::Num(0.0));
            for k in ["__evalt", "__evctrl", "__evshift", "__evmeta"] {
                put(k, Value::Bool(false));
            }
            put("__evrelated", Value::Null);
            // `detail` is 1 for a single click (UI Events §5.3).
            put(EV_DETAIL, Value::Num(1.0));
        }
    }
    let prevented = deliver(i, &ev, kind, chain)?;
    // A click on a `<label>` activates its control (HTML §4.10.4); the text
    // next to a checkbox is its click target.
    //
    // Not if someone canceled, and not if the control itself is already in
    // the path (`<label><input></label>` would otherwise get two clicks and
    // toggle twice).
    if kind == "click" && !prevented {
        if let Some(target) = chain.last().copied() {
            if let Some((lab, ctl)) = label_target(i, target) {
                let _ = lab;
                if !chain.contains(&ctl) {
                    let c = ancestors(i, ctl);
                    let toggles = is_box_control(i, ctl);
                    let before = checked_now(i, ctl);
                    if toggles { set_checked(i, ctl, !before); }
                    let stopped = deliver_click(i, &c)?;
                    if toggles {
                        if stopped { set_checked(i, ctl, before); }
                        else { dispatch_plain(i, "input", &c)?; dispatch_plain(i, "change", &c)?; }
                    }
                }
            }
        }
    }
    // A click is a task: the microtask queue runs afterwards, as after any
    // other. Otherwise a `.then` from the handler would wait for the next
    // timer, and forever on a page without timers.
    super::promise::run_jobs(i);
    Ok(prevented)
}

/// The `<label>` at or above the hit node, and its control.
pub fn label_target(i: &Interp, id: u32) -> Option<(u32, u32)> {
    let d = i.doc.as_ref()?;
    let mut cur = Some(id);
    while let Some(n) = cur {
        if &*d.nodes.get(n as usize)?.tag == "label" {
            return label_control(i, n).map(|c| (n, c));
        }
        cur = d.nodes[n as usize].parent;
    }
    None
}

fn is_box_control(i: &Interp, id: u32) -> bool {
    let Some(d) = i.doc.as_ref() else { return false };
    let Some(n) = d.nodes.get(id as usize) else { return false };
    &*n.tag == "input"
        && n.attr("type").is_some_and(|t| {
            let t = t.to_ascii_lowercase();
            t == "checkbox" || t == "radio"
        })
}

fn deliver_click(i: &mut Interp, chain: &[u32]) -> C<bool> {
    let proto = i.realm.event_proto.clone();
    let ev = build_event(i, proto, "click", true);
    ev.borrow_mut().define(EV_BUBBLES, Prop { value: Some(Value::Bool(true)), get: None,
        set: None, writable: true, enumerable: false, configurable: true });
    deliver(i, &ev, "click", chain)
}

fn dispatch_plain(i: &mut Interp, kind: &str, chain: &[u32]) -> C<bool> {
    let proto = i.realm.event_proto.clone();
    let ev = build_event(i, proto, kind, true);
    ev.borrow_mut().define(EV_BUBBLES, Prop { value: Some(Value::Bool(true)), get: None,
        set: None, writable: true, enumerable: false, configurable: true });
    deliver(i, &ev, kind, chain)
}

/// The shared core: a finished event over a finished path. beak's own
/// clicks and the page's `el.dispatchEvent(…)` both end here, so there is
/// one set of rules.
fn deliver(i: &mut Interp, ev: &Gc, kind: &str, chain: &[u32]) -> C<bool> {
    if chain.is_empty() { return Ok(false); }
    let target = wrap(i, chain[chain.len() - 1]);
    let set = |o: &Gc, k: &str, v: Value| {
        o.borrow_mut().define(k, Prop { value: Some(v), get: None, set: None,
            writable: true, enumerable: false, configurable: true });
    };
    set(ev, EV_TARGET, target);
    let evv = Value::Obj(ev.clone());

    for (k, &node) in chain.iter().enumerate().rev() {
        let mut listeners: Vec<Value> = Vec::new();
        // The handler from the attribute or property runs first: it precedes any
        // `addEventListener` a script registers later, and registration order is
        // call order.
        //
        // Either-or: `el.onclick = f` replaces the attribute handler, as it is the
        // same slot.
        let prop = i.doc.as_ref().and_then(|d| d.nodes[node as usize].handlers.iter()
            .find(|(k, _)| &**k == kind).map(|(_, f)| f.clone()));
        match prop {
            Some(f) => listeners.push(f),
            None => if let Some(f) = inline_handler(i, node, kind)? { listeners.push(f); },
        }
        if let Some(d) = &i.doc {
            listeners.extend(d.nodes[node as usize].listeners.iter()
                .filter(|(k, _)| &**k == kind).map(|(_, f)| f.clone()));
        }
        if listeners.is_empty() { continue; }
        let this_node = wrap(i, node);
        set(ev, EV_CUR, this_node.clone());
        // 2 = AT_TARGET, 3 = BUBBLING_PHASE. There is no capture phase:
        // `addEventListener` accepts the third argument and ignores it.
        set(ev, EV_PHASE, Value::Num(if k + 1 == chain.len() { 2.0 } else { 3.0 }));
        for f in listeners {
            // A throwing handler must not take the following ones with it, as in
            // browsers.
            let r = i.call(&f, this_node.clone(), &[evv.clone()]);
            // A throwing handler is reported to the console, not dropped.
            if let Err(e) = r {
                let msg = super::modules::describe(i, e);
                i.console_push(alloc::format!("Fehler im {kind}-Behandler: {msg}"));
                continue;
            }
            // `onclick="return false"` is the old form of `preventDefault`. It applies
            // only to the attribute handler; `addEventListener` ignores the return
            // value.
            if matches!(r, Ok(Value::Bool(false))) { set(ev, EV_PREVENTED, Value::Bool(true)); }
            let imm = matches!(ev.borrow().get_own(EV_STOPIMM).and_then(|p| p.value.clone()),
                               Some(Value::Bool(true)));
            if imm { break; }
        }
        let stop = matches!(ev.borrow().get_own(EV_STOP).and_then(|p| p.value.clone()),
                            Some(Value::Bool(true)));
        if stop { break; }
    }
    set(ev, EV_CUR, Value::Null);
    set(ev, EV_PHASE, Value::Num(0.0));
    Ok(matches!(ev.borrow().get_own(EV_PREVENTED).and_then(|p| p.value.clone()),
                Some(Value::Bool(true))))
}

/// `theme` -> `data-theme`, `myKey` -> `data-my-key`. The inverse of
/// `dash_to_camel`, used by `el.dataset.x = v`.
pub fn camel_to_data_attr(s: &str) -> String {
    let mut out = String::from("data-");
    for c in s.chars() {
        if c.is_ascii_uppercase() { out.push('-'); out.extend(c.to_lowercase()); }
        else { out.push(c) }
    }
    out
}

/// `data-foo-bar` -> `fooBar`.
fn dash_to_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = false;
    for c in s.chars() {
        if c == '-' { up = true; continue }
        if up { out.extend(c.to_uppercase()); up = false } else { out.push(c) }
    }
    out
}

fn find_tag(d: &Doc, tag: &str) -> Option<u32> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    all.into_iter().find(|&x| &*d.nodes[x as usize].tag == tag)
}

#[cfg(test)]
mod beak_engine_layout_boxes {
    use crate::layout::ElemRect;

    /// A box as layout records it, with only the fields the tests care about.
    pub fn boxed(seq: u32, x: i32, y: i32, w: i32, h: i32, bx: i16, by: i16) -> ElemRect {
        ElemRect { seq, x, y, w, h, bx, by, px: 0, py: 0, positioned: false }
    }

    /// The same box, positioned; only `offsetParent` asks.
    pub fn placed(seq: u32, x: i32, y: i32, w: i32, h: i32) -> ElemRect {
        ElemRect { seq, x, y, w, h, bx: 0, by: 0, px: 0, py: 0, positioned: true }
    }

    /// The same box with padding; only the observer tests need it.
    pub fn padded(seq: u32, x: i32, y: i32, w: i32, h: i32,
                  bx: i16, by: i16, px: i16, py: i16) -> ElemRect {
        ElemRect { seq, x, y, w, h, bx, by, px, py, positioned: false }
    }

    pub fn find_seq(el: &crate::dom::Element, id: &str) -> Option<u32> {
        if el.attr("id") == Some(id) { return Some(el.seq) }
        el.children.iter().find_map(|c| match c {
            crate::dom::Node::Element(e) => find_seq(e, id),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    /// `getComputedStyle` answers from the cascade, not the inline style:
    /// `.hide{display:none}` lives in a stylesheet, not on the element.
    fn run(html: &str, js: &str) -> alloc::vec::Vec<alloc::string::String> {
        let dom = crate::dom::parse(html);
        let media = crate::css::Media::new(1024.0, false);
        let sheet = crate::css::collect_all(&dom, "", media);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_style_context(super::super::interp::StyleCtx {
            sheet: alloc::rc::Rc::new(sheet),
            theme: crate::layout::Theme {
                bg: crate::layout::Rgb(255, 255, 255),
                text: crate::layout::Rgb(33, 37, 41),
                heading: crate::layout::Rgb(33, 37, 41),
                link: crate::layout::Rgb(13, 110, 253),
                muted: crate::layout::Rgb(108, 117, 125),
                rule: crate::layout::Rgb(222, 226, 230),
            },
            viewport_w: 1024.0,
        });
        let prog = super::super::parse(js, false).expect("parst");
        let _ = i.run_program(&prog);
        i.take_console()
    }

    #[test]
    fn computed_style_answers_from_the_cascade() {
        let out = run(
            "<html><head><style>.h{display:none} .c{color:#0d6efd;font-size:20px}</style></head>\
             <body><p class='h' id='a'>x</p><p class='c' id='b'>y</p></body></html>",
            "console.log(getComputedStyle(document.getElementById('a')).display);\
             console.log(getComputedStyle(document.getElementById('b')).color);\
             console.log(getComputedStyle(document.getElementById('b')).fontSize);",
        );
        assert_eq!(out, ["none", "rgb(13, 110, 253)", "20px"]);
    }

    /// `new Image()` is a real `img` element that arrives in the tree and
    /// behaves like one, not a stub.
    #[test]
    fn new_image_ist_ein_img_element_im_baum() {
        let out = run(
            "<html><body></body></html>",
            "var i = new Image();\
             console.log(i.tagName + ' ' + i.nodeType + ' ' + (i instanceof HTMLImageElement));\
             i.src = '/pixel.gif';\
             console.log(i.getAttribute('src'));\
             var j = new Image(16, 9);\
             console.log(j.getAttribute('width') + 'x' + j.getAttribute('height'));\
             document.body.appendChild(i);\
             console.log(String(document.getElementsByTagName('img').length));",
        );
        assert_eq!(out, ["IMG 1 true", "/pixel.gif", "16x9", "1"]);
    }

    /// On the root element `clientWidth` is the viewport (CSSOM View §4), not
    /// the padding box.
    ///
    /// Built by hand instead of `run`: that helper sets no viewport, which is
    /// the subject here.
    #[test]
    fn clientwidth_am_wurzelelement_ist_die_sichtflaeche() {
        let dom = crate::dom::parse("<html><body><p id='p'>x</p></body></html>");
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_media(1902.0, 1000.0, false);
        let src = "[document.documentElement.clientWidth, \
                    document.documentElement.clientHeight, \
                    document.documentElement.clientWidth === window.innerWidth, \
                    document.getElementById('p').clientWidth === window.innerWidth].join(',')";
        let prog = crate::js::parse(src, false).expect("parst");
        let Ok(v) = i.run_program(&prog) else { panic!("der Lauf hat geworfen") };
        let Ok(got) = i.to_string(&v) else { panic!("kein Text") };
        assert_eq!(&*got, "1902,1000,true,false");
    }

    /// An element created by script is not in the tree, so it has no computed
    /// style; the empty string is the honest answer.
    #[test]
    fn an_element_the_script_made_has_no_cascade_yet() {
        let out = run(
            "<html><body><p>x</p></body></html>",
            "var e = document.createElement('div');\
             console.log(JSON.stringify(getComputedStyle(e).display));",
        );
        assert_eq!(out, ["\"\""]);
    }

    /// What the script writes to the inline style is visible in the computed
    /// style, so a script that sets and then measures gets its own value back.
    #[test]
    fn a_style_the_script_just_set_is_in_the_computed_answer() {
        let out = run(
            "<html><head><style>p{color:#000}</style></head><body><p id='a'>x</p></body></html>",
            "var e = document.getElementById('a');\
             console.log(getComputedStyle(e).color);\
             e.style.color = '#1d5c1d';\
             console.log(getComputedStyle(e).color);",
        );
        assert_eq!(out, ["rgb(0, 0, 0)", "rgb(29, 92, 29)"]);
    }

    /// A script sets a class and then measures: the class decides which rules
    /// match, which a tree from script start cannot answer.
    #[test]
    fn a_class_the_script_just_added_decides_the_computed_style() {
        let out = run(
            "<html><head><style>p{color:#000} .an{color:#ff0000;display:none}</style></head>\
             <body><p id=a>x</p></body></html>",
            "var e = document.getElementById('a');\
             console.log(getComputedStyle(e).color + ' ' + getComputedStyle(e).display);\
             e.className = 'an';\
             console.log(getComputedStyle(e).color + ' ' + getComputedStyle(e).display);\
             e.className = '';\
             console.log(getComputedStyle(e).color + ' ' + getComputedStyle(e).display);",
        );
        assert_eq!(out, ["rgb(0, 0, 0) block", "rgb(255, 0, 0) none", "rgb(0, 0, 0) block"]);
    }

    /// A node the script inserts gets its cascade, including what its parent
    /// passes down.
    #[test]
    fn a_node_the_script_appends_gets_the_cascade_of_where_it_landed() {
        let out = run(
            "<html><head><style>.box{color:#0000ff} .box span{color:#008000}</style></head>\
             <body><div class=box id=b></div></body></html>",
            "var s = document.createElement('span');\
             console.log(JSON.stringify(getComputedStyle(s).color));\
             document.getElementById('b').appendChild(s);\
             console.log(getComputedStyle(s).color);",
        );
        assert_eq!(out, ["\"\"", "rgb(0, 128, 0)"]);
    }

    /// Geometry comes from layout, provided by the host. Without boxes the
    /// answer is zeros, as browsers give for an element without a box.
    #[test]
    fn geometry_answers_from_the_boxes_the_host_handed_in() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        let seq = find_seq(&dom.root, "a").expect("das div");
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                boxed(seq, 10, 100, 200, 50, 4, 6),
            ]),
            scroll: (0, 40),
            content: (1024, 768),
        });
        let prog = super::super::parse(
            "var e = document.getElementById('a'), r = e.getBoundingClientRect();\
             console.log([r.x, r.y, r.width, r.height, r.right, r.bottom].join(','));\
             console.log([e.offsetWidth, e.offsetHeight, e.offsetTop].join(','));\
             console.log([e.clientWidth, e.clientHeight].join(','));\
             console.log(e.getClientRects().length);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), [
            // y is shifted by the scroll position: 100 - 40.
            "10,60,200,50,210,110",
            // offsetTop is relative to the document, so without scrolling.
            "200,50,100",
            // Padding box = border box minus the border sums.
            "196,44",
            "1",
        ]);
    }

    /// A box across several lines has several fragments. `getClientRects`
    /// lists them, `getBoundingClientRect` returns their union, not the first
    /// one found.
    #[test]
    fn a_box_broken_over_lines_reports_the_union() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><span id=a>x</span></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        let seq = find_seq(&dom.root, "a").expect("der span");
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                boxed(seq, 100, 10, 50, 20, 0, 0),
                boxed(seq, 10, 30, 90, 20, 0, 0),
            ]),
            scroll: (0, 0),
            content: (1024, 4000),
        });
        let prog = super::super::parse(
            "var r = document.getElementById('a').getBoundingClientRect();\
             console.log([r.left, r.top, r.right, r.bottom].join(','));\
             console.log(document.getElementById('a').getClientRects().length);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["10,10,150,50", "2"]);
    }


    /// `ResizeObserver` reports the content box, with border and padding
    /// subtracted; a chart sizes its canvas from `entry.contentRect.width`.
    #[test]
    fn resize_observer_reports_the_content_box_not_the_border_box() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        let seq = find_seq(&dom.root, "a").expect("das div");
        let prog = super::super::parse(
            "var log = [];\
             var ro = new ResizeObserver(function (es) {\
               for (var k = 0; k < es.length; k++) {\
                 var e = es[k];\
                 log.push(e.contentRect.width + 'x' + e.contentRect.height\
                          + ' rand ' + e.borderBoxSize[0].inlineSize\
                          + ' inhalt ' + e.contentBoxSize[0].blockSize);\
               }\
             });\
             ro.observe(document.getElementById('a'));", false).expect("parst");
        // Boxes first, then the script: `observe` measures immediately.
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                // 200x50 border box, 4 px border and 20 px padding horizontally, 6 + 10
                // vertically.
                padded(seq, 10, 100, 200, 50, 4, 6, 20, 10),
            ]),
            scroll: (0, 0),
            content: (1024, 4000),
        });
        let _ = i.run_program(&prog);
        // Delivered at the checkpoint, not inside `observe`.
        i.run_timers();
        let prog = super::super::parse("console.log(log.join(' | '));", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["176x34 rand 200 inhalt 34"]);
    }

    /// It reports only changes. A second layout with the same size is not an
    /// event.
    #[test]
    fn resize_observer_stays_quiet_when_the_size_did_not_move() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        let seq = find_seq(&dom.root, "a").expect("das div");
        let geom = |w: i32| super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![boxed(seq, 0, 0, w, 50, 0, 0)]),
            scroll: (0, 0),
            content: (1024, 4000),
        };
        i.set_geometry(geom(200));
        let prog = super::super::parse(
            "var n = 0;\
             var ro = new ResizeObserver(function (es) { n += es.length; });\
             ro.observe(document.getElementById('a'));", false).expect("parst");
        let _ = i.run_program(&prog);
        i.run_timers();
        // The same three more times: the size is stable.
        for _ in 0..3 { i.set_geometry(geom(200)); i.run_timers(); }
        // Now it changes.
        i.set_geometry(geom(300));
        i.run_timers();
        let prog = super::super::parse("console.log(n);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["2"]);
    }

    /// `IntersectionObserver`: the ratio is the intersected area divided by
    /// the target's, reported when crossing a threshold.
    #[test]
    fn intersection_observer_reports_when_a_threshold_is_crossed() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_viewport(1000.0, 1000.0);
        let seq = find_seq(&dom.root, "a").expect("das div");
        // A 100 px tall box with its top edge at y. At y = 950, 50 of 100 are in
        // the viewport: ratio 0.5.
        let at = |y: i32| super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![boxed(seq, 0, y, 100, 100, 0, 0)]),
            scroll: (0, 0),
            content: (1024, 4000),
        };
        i.set_geometry(at(2000));
        let prog = super::super::parse(
            "var log = [];\
             var io = new IntersectionObserver(function (es) {\
               for (var k = 0; k < es.length; k++) {\
                 log.push(es[k].isIntersecting + '@' + es[k].intersectionRatio);\
               }\
             }, { threshold: [0, 0.5] });\
             io.observe(document.getElementById('a'));", false).expect("parst");
        let _ = i.run_program(&prog);
        i.run_timers();
        // A quarter in: above 0, below 0.5.
        i.set_geometry(at(975));
        i.run_timers();
        // Half: the second threshold is crossed.
        i.set_geometry(at(950));
        i.run_timers();
        // Out again.
        i.set_geometry(at(2000));
        i.run_timers();
        let prog = super::super::parse("console.log(log.join(' | '));", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(),
                   ["false@0 | true@0.25 | true@0.5 | false@0"]);
    }

    /// `rootMargin` grows the clip; that is what makes "load the image before
    /// it becomes visible" possible.
    #[test]
    fn a_root_margin_stretches_the_clip() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_viewport(1000.0, 1000.0);
        let seq = find_seq(&dom.root, "a").expect("das div");
        // 200 px below the viewport.
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![boxed(seq, 0, 1200, 100, 100, 0, 0)]),
            scroll: (0, 0),
            content: (1024, 4000),
        });
        let prog = super::super::parse(
            "var ohne = null, mit = null;\
             new IntersectionObserver(function (es) { ohne = es[0].isIntersecting; })\
               .observe(document.getElementById('a'));\
             var io = new IntersectionObserver(function (es) { mit = es[0].isIntersecting; },\
                                               { rootMargin: '0px 0px 300px 0px' });\
             io.observe(document.getElementById('a'));", false).expect("parst");
        let _ = i.run_program(&prog);
        i.run_timers();
        let prog = super::super::parse(
            "console.log(ohne + ',' + mit + ',' + io.rootMargin);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["false,true,0px 0px 300px 0px"]);
    }


    /// Scroll metrics come from the boxes. `scrollHeight` is the padding box
    /// united with the descendants; a child overflowing its parent is exactly
    /// what pages ask about.
    #[test]
    fn scroll_metrics_come_from_the_boxes_not_from_zero() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a><p id=b>x</p></div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_media(1000.0, 800.0, false);
        let a = find_seq(&dom.root, "a").expect("a");
        let b = find_seq(&dom.root, "b").expect("b");
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                // 200x60 border box, 4 px border and 10 px padding.
                padded(a, 0, 0, 200, 60, 4, 4, 10, 10),
                // The child is 400 tall and overflows at the bottom.
                boxed(b, 2, 2, 180, 400, 0, 0),
            ]),
            scroll: (0, 0),
            content: (1000, 3000),
        });
        let prog = super::super::parse(
            "var a = document.getElementById('a');\
             console.log([a.clientWidth, a.clientHeight].join(','));\
             console.log([a.scrollWidth, a.scrollHeight].join(','));\
             console.log([a.scrollTop, a.scrollLeft].join(','));", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), [
            // Padding box: 200-4, 60-4.
            "196,56",
            // Scrollable area: the width fits (the child ends at 182, the padding box
            // is 196 wide), the height does not: the child ends at 402, the padding
            // edge is at 2.
            "196,400",
            // beak clips nothing: on an ordinary element nothing is scrolled, and 0
            // is true here.
            "0,0",
        ]);
    }

    /// On the root element it is the document's scrollable area as layout
    /// reports it, not the union of boxes.
    #[test]
    fn the_root_reports_the_documents_scrolling_area() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><p id=p>x</p></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_media(1000.0, 800.0, false);
        let p = find_seq(&dom.root, "p").expect("p");
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![boxed(p, 0, 0, 100, 20, 0, 0)]),
            scroll: (0, 250),
            content: (1000, 3000),
        });
        let prog = super::super::parse(
            "var r = document.documentElement;\
             console.log([r.scrollHeight, r.scrollWidth].join(','));\
             console.log([r.scrollTop, window.scrollY, window.pageYOffset].join(','));\
             console.log(document.scrollingElement === r);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["3000,1000", "250,250,250", "true"]);
    }

    /// Scrolling is a request. The engine has no window; it records what the
    /// page wanted and the host collects it.
    #[test]
    fn scrolling_is_a_request_the_host_picks_up() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><p id=p>x</p></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_media(1000.0, 800.0, false);
        let p = find_seq(&dom.root, "p").expect("p");
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![boxed(p, 0, 1200, 100, 20, 0, 0)]),
            scroll: (0, 0),
            content: (1000, 3000),
        });
        let prog = super::super::parse("window.scrollTo({ top: 400 });", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_scroll(), Some((None, Some(400.0))));
        // `scrollIntoView` computes the document position, not the viewport
        // position: the box is at 1200, nothing is scrolled.
        let prog = super::super::parse(
            "document.getElementById('p').scrollIntoView();", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_scroll(), Some((None, Some(1200.0))));
        // Collecting twice yields nothing the second time.
        assert_eq!(i.take_scroll(), None);
    }

    /// `offsetParent` is the nearest positioned ancestor; the flag travels
    /// with the layout box.
    #[test]
    fn offset_parent_finds_the_nearest_positioned_ancestor() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=aussen><div id=mitte>\
                    <span id=innen>x</span></div></div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        let (a, m, n) = (find_seq(&dom.root, "aussen").expect("a"),
                         find_seq(&dom.root, "mitte").expect("m"),
                         find_seq(&dom.root, "innen").expect("n"));
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                placed(a, 0, 0, 300, 300),          // positioned
                boxed(m, 0, 0, 300, 200, 0, 0),     // not positioned
                boxed(n, 0, 0, 50, 20, 0, 0),
            ]),
            scroll: (0, 0),
            content: (1000, 3000),
        });
        let prog = super::super::parse(
            "console.log(document.getElementById('innen').offsetParent.id);\
             console.log(document.getElementById('aussen').offsetParent.tagName);\
             console.log(document.body.offsetParent);\
             console.log(document.createElement('div').offsetParent);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["aussen", "BODY", "null", "null"]);
    }

    /// Small members in one run; each was missing before.
    #[test]
    fn the_small_gaps_answer_instead_of_throwing() {
        let out = run("<html><body><a id=l href='/x#t' data-k=v>l</a><b id=b>b</b>\
                       <img id=i src='/p.png'></body></html>",
            "var l = document.getElementById('l');\
             console.log(l.attributes.length + ',' + l.attributes[0].name + ','\
                       + l.attributes.getNamedItem('data-k').value);\
             console.log(l.toggleAttribute('open') + ',' + l.hasAttribute('open') + ','\
                       + l.toggleAttribute('open') + ',' + l.hasAttribute('open'));\
             console.log(l.nextElementSibling.id + ','\
                       + document.getElementById('i').previousElementSibling.id);\
             console.log(l.isConnected + ',' + document.createElement('i').isConnected);\
             console.log(document.body.childElementCount + ','\
                       + document.body.lastElementChild.tagName);\
             console.log(l.hash + ',' + document.getElementById('i').currentSrc);\
             l.ariaHidden = 'true';\
             console.log(l.ariaHidden + ',' + l.getAttribute('aria-hidden'));\
             l.ariaHidden = null;\
             console.log(l.ariaHidden + ',' + l.hasAttribute('aria-hidden'));\
             var d = document.createElement('div');\
             d.innerHTML = '<i>1</i><b>2</b>';\
             d.replaceChildren(document.createElement('u'));\
             console.log(d.childNodes.length + ',' + d.firstElementChild.tagName);\
             console.log(Object.prototype.toString.call(l.attributes) + ','\
                       + Object.prototype.toString.call(l.attributes[0]));\
             var u = new URL('http://bob:x@example.com/p');\
             console.log('[' + u.username + '][' + u.password + ']' + u.href);");
        assert_eq!(out, [
            "3,id,v",
            "true,true,false,false",
            "b,b",
            "true,false",
            "3,IMG",
            "#t,/p.png",
            "true,true",
            "null,false",
            "1,U",
            "[object NamedNodeMap],[object Attr]",
            // beak does not support credentials and says so, instead of staying
            // silent or putting them in the address bar.
            "[][]http://example.com/p",
        ]);
    }

    /// The inline style stays a live view; `el.style` differs from
    /// `getComputedStyle(el)`, and both go through the same accessors.
    #[test]
    fn the_inline_view_still_writes_through() {
        let out = run(
            "<html><body><p id='a'>x</p></body></html>",
            "var e = document.getElementById('a');\
             e.style.color = 'red';\
             console.log(e.style.color + '|' + e.getAttribute('style'));",
        );
        assert_eq!(out, ["red|color: red;"]);
    }
}

/// Internal field of a `TextDecoder`: drop an invalid sequence silently,
/// or throw? NUL-prefixed, so invisible to scripts.
const TD_FATAL: &str = "\0!tdfatal";

/// `TextEncoder` and `TextDecoder`.
///
/// UTF-8 only. The spec gives the encoder no other choice, and the decoder
/// accepts labels, but any other encoding needs tables not present here.
/// A foreign label is accepted and treated as UTF-8 rather than throwing.
fn install_text_codec(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let op = realm.object_proto.clone();

    // ── TextEncoder ──────────────────────────────────────────────────────
    let enc_proto = new_obj(Some(op.clone()));
    getter(&enc_proto, "encoding", |_, _, _| Ok(Value::str("utf-8")), &fp);
    meth(&enc_proto, "encode", |i, _, a| {
        let s = match a.first() {
            None | Some(Value::Undefined) => Rc::from(""),
            Some(v) => i.to_string(v)?,
        };
        Ok(bytes_to_u8(i, s.as_bytes()))
    }, 1, &fp);
    // `encodeInto` writes into an existing view and reports how far it got.
    // It cuts at a character boundary; half a sequence would be broken UTF-8.
    meth(&enc_proto, "encodeInto", |i, _, a| {
        let s = match a.first() {
            None | Some(Value::Undefined) => Rc::from(""),
            Some(v) => i.to_string(v)?,
        };
        let Some(dst) = a.get(1).cloned() else { return i.type_err("encodeInto needs a target") };
        let cap = view_len(&dst);
        let mut written = 0usize;
        let mut read = 0usize;
        for c in s.chars() {
            let n = c.len_utf8();
            if written + n > cap { break }
            written += n;
            read += c.len_utf16();
        }
        let bytes = &s.as_bytes()[..written];
        write_view(&dst, bytes);
        let out = new_obj(Some(i.realm.object_proto.clone()));
        out.borrow_mut().define("read", Prop::data(Value::Num(read as f64)));
        out.borrow_mut().define("written", Prop::data(Value::Num(written as f64)));
        Ok(Value::Obj(out))
    }, 2, &fp);
    let enc_ctor = native(Some(fp.clone()), |i, _, _| {
        Ok(Value::Obj(new_obj(Some(i.realm.text_encoder_proto.clone()))))
    }, "TextEncoder", 0, true);
    enc_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(enc_proto.clone())));
    enc_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(enc_ctor.clone())));
    enc_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("TextEncoder")));
    enc_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("TextEncoder")));
    realm.global.borrow_mut().define("TextEncoder", Prop::builtin(Value::Obj(enc_ctor)));
    realm.text_encoder_proto = enc_proto;

    // ── TextDecoder ──────────────────────────────────────────────────────
    let dec_proto = new_obj(Some(op));
    getter(&dec_proto, "encoding", |_, _, _| Ok(Value::str("utf-8")), &fp);
    getter(&dec_proto, "fatal", |i, t, _| {
        Ok(Value::Bool(matches!(i.get(&t, TD_FATAL)?, Value::Bool(true))))
    }, &fp);
    meth(&dec_proto, "decode", |i, t, a| {
        let src = a.first().cloned().unwrap_or(Value::Undefined);
        if matches!(src, Value::Undefined) { return Ok(Value::str("")); }
        let bytes = read_view(&src);
        match core::str::from_utf8(&bytes) {
            Ok(s) => Ok(Value::str(s)),
            Err(_) => {
                if matches!(i.get(&t, TD_FATAL)?, Value::Bool(true)) {
                    return i.type_err("The encoded data was not valid utf-8");
                }
                // Without `fatal` the spec replaces each invalid sequence with U+FFFD
                // instead of throwing.
                Ok(Value::string(lossy_utf8(&bytes)))
            }
        }
    }, 1, &fp);
    let dec_ctor = native(Some(fp.clone()), |i, _, a| {
        let fatal = match a.get(1) {
            Some(o @ Value::Obj(_)) => i.get(o, "fatal")?.truthy(),
            _ => false,
        };
        let g = new_obj(Some(i.realm.text_decoder_proto.clone()));
        g.borrow_mut().define(TD_FATAL, Prop::frozen(Value::Bool(fatal)));
        Ok(Value::Obj(g))
    }, "TextDecoder", 0, true);
    dec_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(dec_proto.clone())));
    dec_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(dec_ctor.clone())));
    dec_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("TextDecoder")));
    dec_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("TextDecoder")));
    realm.global.borrow_mut().define("TextDecoder", Prop::builtin(Value::Obj(dec_ctor)));
    realm.text_decoder_proto = dec_proto;
}

/// A fresh `Uint8Array` with these bytes.
fn bytes_to_u8(i: &mut Interp, b: &[u8]) -> Value {
    let v = i.new_typed(ElemKind::U8, b.len());
    write_view(&v, b);
    v
}

/// The bytes behind a view or a buffer. Anything else is empty.
fn read_view(v: &Value) -> Vec<u8> {
    let Value::Obj(o) = v else { return Vec::new() };
    // Extract the fields first, then drop the borrow: `slice_of` borrows the
    // buffer again, and for `ObjKind::Buffer` that is the same object.
    let what = match &o.borrow().kind {
        ObjKind::Buffer(b) => return b.bytes.borrow().clone(),
        ObjKind::TypedArray(td) => (td.buf.clone(), td.offset, td.len * td.kind.size()),
        ObjKind::DataView(dv) => (dv.buf.clone(), dv.offset, dv.len),
        _ => return Vec::new(),
    };
    slice_of(&what.0, what.1, what.2)
}

fn slice_of(buf: &Gc, off: usize, len: usize) -> Vec<u8> {
    let ObjKind::Buffer(b) = &buf.borrow().kind else { return Vec::new() };
    let all = b.bytes.borrow();
    if off > all.len() { return Vec::new() }
    all[off..(off + len).min(all.len())].to_vec()
}

/// How many bytes fit into the view.
fn view_len(v: &Value) -> usize {
    let Value::Obj(o) = v else { return 0 };
    match &o.borrow().kind {
        ObjKind::TypedArray(td) => td.len * td.kind.size(),
        ObjKind::DataView(dv) => dv.len,
        ObjKind::Buffer(b) => b.bytes.borrow().len(),
        _ => 0,
    }
}

fn write_view(v: &Value, src: &[u8]) {
    let Value::Obj(o) = v else { return };
    let (buf, off, cap) = match &o.borrow().kind {
        ObjKind::TypedArray(td) => (td.buf.clone(), td.offset, td.len * td.kind.size()),
        ObjKind::DataView(dv) => (dv.buf.clone(), dv.offset, dv.len),
        ObjKind::Buffer(_) => (o.clone(), 0, usize::MAX),
        _ => return,
    };
    let ObjKind::Buffer(b) = &buf.borrow().kind else { return };
    let mut all = b.bytes.borrow_mut();
    let n = src.len().min(cap).min(all.len().saturating_sub(off));
    all[off..off + n].copy_from_slice(&src[..n]);
}

/// UTF-8 with U+FFFD for each invalid sequence.
fn lossy_utf8(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut rest = b;
    loop {
        match core::str::from_utf8(rest) {
            Ok(s) => { out.push_str(s); return out }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(unsafe { core::str::from_utf8_unchecked(&rest[..good]) });
                out.push('\u{FFFD}');
                match e.error_len() {
                    Some(n) => rest = &rest[good + n..],
                    None => return out,
                }
            }
        }
    }
}

// ── Custom elements (HTML §4.13) ────────────────────────────────────────────

/// `DomNode::ce`: no definition applied yet.
pub const CE_NONE: u8 = 0;
/// An upgrade is running the constructor.
pub const CE_RUNNING: u8 = 1;
pub const CE_CUSTOM: u8 = 2;
pub const CE_FAILED: u8 = 3;
/// On a fragment: template contents, which are never upgraded.
pub const CE_INERT: u8 = 4;

/// `:defined`: "uncustomized" or "custom" (HTML §4.16.3).
pub fn ce_defined(n: &DomNode) -> bool {
    n.ce == CE_CUSTOM || (n.ce == CE_NONE && !crate::dom::valid_custom_element_name(&n.tag))
}

/// The tag of a shadow root node.
pub const SHADOW_TAG: &str = "#shadow-root";

/// One `customElements.define` (HTML §4.13.4 "custom element definition").
/// The callbacks are read once, at definition, as the spec requires.
pub struct CeDef {
    pub name: Rc<str>,
    pub ctor: Value,
    pub proto: Gc,
    pub observed: Vec<Rc<str>>,
    pub connected: Value,
    pub disconnected: Value,
    pub adopted: Value,
    pub attr_changed: Value,
    pub form_associated: bool,
}

#[derive(Default)]
pub struct CeRegistry {
    pub defs: Vec<CeDef>,
    /// `whenDefined` promises still waiting, by name.
    pub waiting: Vec<(Rc<str>, Gc)>,
    /// The construction stack: definition index and the element an upgrade
    /// constructs; `None` once `super()` has handed the element out.
    pub stack: Vec<(usize, Option<u32>)>,
    /// `define` is reading the class; a nested `define` throws.
    pub defining: bool,
}

impl CeRegistry {
    pub fn by_name(&self, name: &str) -> Option<usize> {
        self.defs.iter().position(|d| &*d.name == name)
    }

    /// Every value the registry keeps alive.
    pub fn values(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for d in &self.defs {
            out.push(d.ctor.clone());
            out.push(Value::Obj(d.proto.clone()));
            for v in [&d.connected, &d.disconnected, &d.adopted, &d.attr_changed] {
                out.push(v.clone());
            }
        }
        for (_, p) in &self.waiting { out.push(Value::Obj(p.clone())); }
        out
    }
}

/// A `DOMException` with the given name (WebIDL §3.14.1).
pub fn dom_exc(i: &mut Interp, name: &str, msg: &str) -> Abrupt {
    let proto = i.realm.global.borrow().get_own("DOMException")
        .and_then(|p| p.value.clone())
        .and_then(|c| match c { Value::Obj(c) => c.borrow().get_own("prototype").and_then(|p| p.value.clone()), _ => None });
    let e = match proto {
        Some(Value::Obj(p)) => new_kind(Some(p), ObjKind::Error),
        _ => return i.throw_kind("Error", msg),
    };
    {
        let mut b = e.borrow_mut();
        b.define("message", Prop::builtin(Value::string(String::from(msg))));
        b.define("name", Prop::builtin(Value::string(String::from(name))));
        b.define("code", Prop::builtin(Value::Num(dom_exc_code(name))));
    }
    Abrupt::Throw(Value::Obj(e))
}

/// The legacy code of a `DOMException` name (WebIDL §3.14.2), 0 for new names.
fn dom_exc_code(name: &str) -> f64 {
    const CODES: &[(&str, f64)] = &[
        ("IndexSizeError", 1.0), ("HierarchyRequestError", 3.0), ("WrongDocumentError", 4.0),
        ("InvalidCharacterError", 5.0), ("NoModificationAllowedError", 7.0),
        ("NotFoundError", 8.0), ("NotSupportedError", 9.0), ("InvalidStateError", 11.0),
        ("SyntaxError", 12.0), ("InvalidModificationError", 13.0), ("NamespaceError", 14.0),
        ("InvalidAccessError", 15.0), ("TypeMismatchError", 17.0), ("SecurityError", 18.0),
        ("NetworkError", 19.0), ("AbortError", 20.0), ("URLMismatchError", 21.0),
        ("QuotaExceededError", 22.0), ("TimeoutError", 23.0), ("InvalidNodeTypeError", 24.0),
        ("DataCloneError", 25.0),
    ];
    CODES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or(0.0)
}

fn install_dom_exception(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.error_proto.clone()));
    let ctor = native(Some(fp.clone()), |i, this, a| {
        if !i.native_new { return i.type_err("Constructor DOMException requires 'new'") }
        let msg = match a.first() { None | Some(Value::Undefined) => String::new(),
                                    Some(v) => i.to_string(v)?.to_string() };
        let name = match a.get(1) { None | Some(Value::Undefined) => String::from("Error"),
                                    Some(v) => i.to_string(v)?.to_string() };
        let Abrupt::Throw(e) = dom_exc(i, &name, &msg) else { return Ok(Value::Undefined) };
        // A subclass gets its own prototype through the receiver.
        if let (Value::Obj(e), Value::Obj(t)) = (&e, &this) {
            let p = t.borrow().proto.clone();
            if p.is_some() { e.borrow_mut().proto = p; }
        }
        Ok(e)
    }, "DOMException", 0, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("DOMException")));
    for (n, c) in [("INDEX_SIZE_ERR", 1.0), ("NOT_FOUND_ERR", 8.0), ("NOT_SUPPORTED_ERR", 9.0),
                   ("INVALID_STATE_ERR", 11.0), ("SYNTAX_ERR", 12.0), ("SECURITY_ERR", 18.0),
                   ("NETWORK_ERR", 19.0), ("ABORT_ERR", 20.0), ("TIMEOUT_ERR", 23.0),
                   ("DATA_CLONE_ERR", 25.0)] {
        ctor.borrow_mut().define(n, Prop::frozen(Value::Num(c)));
        proto.borrow_mut().define(n, Prop::frozen(Value::Num(c)));
    }
    realm.global.borrow_mut().define("DOMException", Prop::builtin(Value::Obj(ctor)));
}

/// `customElements` and the `HTMLElement` constructor.
///
/// Not implemented: customized built-in elements (`extends`, `is`); the
/// option is accepted and ignored.
fn install_custom_elements(realm: &mut Realm, html_element_proto: &Gc) {
    let fp = realm.function_proto.clone();

    // HTML §3.2.3 "HTML element constructors". The class comes from the
    // receiver's prototype, which `super()` and `Reflect.construct` set to
    // the new target's.
    let he = native(Some(fp.clone()), |i, this, _| {
        if !i.native_new { return i.type_err("Illegal constructor"); }
        let Some(k) = ce_def_of(i, &this) else {
            return i.type_err("Illegal constructor: the class is not a registered custom element");
        };
        let proto = match &this { Value::Obj(o) => o.borrow().proto.clone(), _ => None }
            .unwrap_or_else(|| i.custom.defs[k].proto.clone());
        // An upgrade: the element is already there and waits on the stack.
        if let Some(pos) = i.custom.stack.iter().rposition(|(d, _)| *d == k) {
            let Some(id) = i.custom.stack[pos].1.take() else {
                return Err(dom_exc(i, "InvalidStateError",
                    "the custom element constructor called super() twice"));
            };
            let el = wrap(i, id);
            if let Value::Obj(o) = &el { o.borrow_mut().proto = Some(proto); }
            return Ok(el);
        }
        let name = i.custom.defs[k].name.clone();
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &name);
        d.nodes[id as usize].ce = CE_CUSTOM;
        let el = wrap(i, id);
        if let Value::Obj(o) = &el { o.borrow_mut().proto = Some(proto); }
        Ok(el)
    }, "HTMLElement", 0, true);
    he.borrow_mut().define("prototype", Prop::frozen(Value::Obj(html_element_proto.clone())));
    html_element_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(he.clone())));
    realm.global.borrow_mut().define("HTMLElement", Prop::builtin(Value::Obj(he)));

    let ce = new_obj(Some(realm.object_proto.clone()));
    meth(&ce, "define", |i, _, a| {
        let name: Rc<str> = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let ctor = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !i.is_constructor(&ctor) {
            return i.type_err("customElements.define: the second argument is not a constructor");
        }
        if !crate::dom::valid_custom_element_name(&name) {
            return Err(dom_exc(i, "SyntaxError",
                &alloc::format!("'{name}' is not a valid custom element name")));
        }
        if i.custom.by_name(&name).is_some() {
            return Err(dom_exc(i, "NotSupportedError",
                &alloc::format!("the name '{name}' has already been defined")));
        }
        if i.custom.defs.iter().any(|d| d.ctor.strict_eq(&ctor)) {
            return Err(dom_exc(i, "NotSupportedError",
                "this constructor has already been used with another name"));
        }
        if i.custom.defining {
            return Err(dom_exc(i, "NotSupportedError", "customElements.define is already running"));
        }
        i.custom.defining = true;
        let read = ce_read_class(i, &name, &ctor);
        i.custom.defining = false;
        let def = read?;
        i.custom.defs.push(def);
        let k = i.custom.defs.len() - 1;
        if let Some(d) = &mut i.doc { d.ce_on = true; }
        // Upgrade candidates: the elements of that name in the document, in
        // shadow-including tree order.
        let mut cands = Vec::new();
        if let Some(d) = &i.doc { ce_walk(d, d.doc, &mut cands); }
        cands.retain(|&n| i.doc.as_ref().is_some_and(|d|
            d.nodes[n as usize].ce == CE_NONE && *d.nodes[n as usize].tag == *name));
        for n in cands { ce_upgrade(i, n, k); }
        let ctor = i.custom.defs[k].ctor.clone();
        while let Some(pos) = i.custom.waiting.iter().position(|(t, _)| *t == name) {
            let (_, p) = i.custom.waiting.remove(pos);
            super::promise::resolve_promise(i, &p, ctor.clone());
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&ce, "get", |i, _, a| {
        let name = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(i.custom.by_name(&name).map(|k| i.custom.defs[k].ctor.clone())
            .unwrap_or(Value::Undefined))
    }, 1, &fp);
    meth(&ce, "getName", |i, _, a| {
        let c = a.first().cloned().unwrap_or(Value::Undefined);
        Ok(i.custom.defs.iter().find(|d| d.ctor.strict_eq(&c))
            .map(|d| Value::Str(d.name.clone())).unwrap_or(Value::Null))
    }, 1, &fp);
    meth(&ce, "whenDefined", |i, _, a| {
        let name: Rc<str> = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        if !crate::dom::valid_custom_element_name(&name) {
            let p = super::promise::new_promise(i);
            let Abrupt::Throw(e) = dom_exc(i, "SyntaxError",
                &alloc::format!("'{name}' is not a valid custom element name"))
                else { return Ok(Value::Obj(p)) };
            super::promise::settle(i, &p, e, true);
            return Ok(Value::Obj(p));
        }
        if let Some(k) = i.custom.by_name(&name) {
            let p = super::promise::new_promise(i);
            let c = i.custom.defs[k].ctor.clone();
            super::promise::resolve_promise(i, &p, c);
            return Ok(Value::Obj(p));
        }
        // The same promise for every call until the name is defined.
        if let Some((_, p)) = i.custom.waiting.iter().find(|(t, _)| *t == name) {
            return Ok(Value::Obj(p.clone()));
        }
        let p = super::promise::new_promise(i);
        i.custom.waiting.push((name, p.clone()));
        Ok(Value::Obj(p))
    }, 1, &fp);
    // Upgrade every element below `root`, connected or not.
    meth(&ce, "upgrade", |i, _, a| {
        let root = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        let mut list = Vec::new();
        if let Some(d) = &i.doc { list.push(root); ce_walk(d, root, &mut list); }
        for n in list { ce_try_upgrade(i, n); }
        Ok(Value::Undefined)
    }, 1, &fp);
    ce.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CustomElementRegistry")));
    realm.global.borrow_mut().define("customElements", Prop::builtin(Value::Obj(ce)));
}

/// Read the class for `define` (HTML §4.13.4 steps 14-15): prototype,
/// lifecycle callbacks, `observedAttributes`, `formAssociated`.
fn ce_read_class(i: &mut Interp, name: &Rc<str>, ctor: &Value) -> C<CeDef> {
    let Value::Obj(proto) = i.get(ctor, "prototype")? else {
        return i.type_err("customElements.define: the prototype is not an object");
    };
    let pv = Value::Obj(proto.clone());
    let mut cbs = [Value::Undefined, Value::Undefined, Value::Undefined, Value::Undefined];
    for (k, key) in ["connectedCallback", "disconnectedCallback", "adoptedCallback",
                     "attributeChangedCallback"].iter().enumerate() {
        let v = i.get(&pv, key)?;
        if !matches!(v, Value::Undefined) && !i.is_callable(&v) {
            return i.type_err(&alloc::format!("customElements.define: {key} is not a function"));
        }
        cbs[k] = v;
    }
    let mut observed = Vec::new();
    if !matches!(cbs[3], Value::Undefined) {
        let obs = i.get(ctor, "observedAttributes")?;
        if !matches!(obs, Value::Undefined) {
            for v in i.iterate(&obs)? { observed.push(i.to_string(&v)?); }
        }
    }
    let fa = i.get(ctor, "formAssociated")?;
    let form_associated = fa.truthy();
    let [connected, disconnected, adopted, attr_changed] = cbs;
    Ok(CeDef { name: name.clone(), ctor: ctor.clone(), proto, observed, connected,
               disconnected, adopted, attr_changed, form_associated })
}

/// Which definition does this receiver belong to? The innermost prototype
/// on its chain that is a defined class's prototype, which is the new
/// target's.
fn ce_def_of(i: &Interp, this: &Value) -> Option<usize> {
    let Value::Obj(o) = this else { return None };
    let mut cur = o.borrow().proto.clone();
    while let Some(p) = cur {
        if let Some(k) = i.custom.defs.iter().position(|d| Rc::ptr_eq(&d.proto, &p)) {
            return Some(k);
        }
        let n = p.borrow().proto.clone();
        cur = n;
    }
    None
}

/// The elements below `id` in shadow-including tree order, without `id`.
/// Template contents are skipped: they are never upgraded.
fn ce_walk(d: &Doc, id: u32, out: &mut Vec<u32>) {
    let n = &d.nodes[id as usize];
    if n.kind == ELEMENT_NODE && &*n.tag == "template" { return }
    if let Some(s) = n.shadow {
        if &*n.tag != SHADOW_TAG { ce_walk(d, s, out); }
    }
    for &c in &n.children {
        if d.nodes[c as usize].kind == ELEMENT_NODE && !d.nodes[c as usize].tag.starts_with('#') {
            out.push(c);
        }
        ce_walk(d, c, out);
    }
}

/// `id` and the elements below it.
fn ce_subtree(i: &Interp, id: u32) -> Vec<u32> {
    let Some(d) = &i.doc else { return Vec::new() };
    let mut out = Vec::new();
    if d.nodes[id as usize].kind == ELEMENT_NODE && !d.nodes[id as usize].tag.starts_with('#') {
        out.push(id);
    }
    ce_walk(d, id, &mut out);
    out
}

/// Is `id` inside template contents?
fn ce_inert(d: &Doc, mut id: u32) -> bool {
    loop {
        let n = &d.nodes[id as usize];
        if n.ce == CE_INERT { return true }
        match n.parent {
            Some(p) => {
                if &*d.nodes[p as usize].tag == "template" { return true }
                id = p;
            }
            None => return false,
        }
    }
}

/// Upgrade `id` if it is undefined and its name is defined.
fn ce_try_upgrade(i: &mut Interp, id: u32) {
    let k = {
        let Some(d) = &i.doc else { return };
        let n = &d.nodes[id as usize];
        if n.kind != ELEMENT_NODE || n.ce != CE_NONE || !n.tag.contains('-') { return }
        match i.custom.by_name(&n.tag) { Some(k) => k, None => return }
    };
    ce_upgrade(i, id, k);
}

/// HTML §4.13.5 "upgrade an element": run the constructor on the existing
/// element, then report its attributes and, if connected, the connection.
fn ce_upgrade(i: &mut Interp, id: u32, k: usize) {
    {
        let Some(d) = &mut i.doc else { return };
        if d.nodes[id as usize].ce != CE_NONE { return }
        d.nodes[id as usize].ce = CE_RUNNING;
    }
    let ctor = i.custom.defs[k].ctor.clone();
    i.custom.stack.push((k, Some(id)));
    let r = i.construct(&ctor, &[]);
    i.custom.stack.pop();
    let el = i.doc.as_ref().and_then(|d| d.nodes[id as usize].js.clone());
    let failure = match r {
        Ok(Value::Obj(o)) if el.as_ref().is_some_and(|e| Rc::ptr_eq(e, &o)) => None,
        Ok(_) => Some(String::from(
            "TypeError: the custom element constructor did not produce the element")),
        Err(e) => Some(super::modules::describe(i, e)),
    };
    let name = i.custom.defs[k].name.clone();
    if let Some(msg) = failure {
        if let Some(d) = &mut i.doc { d.nodes[id as usize].ce = CE_FAILED; d.touch(); }
        i.console_push(alloc::format!("custom element <{name}>: upgrade failed: {msg}"));
        return;
    }
    let attrs = match &mut i.doc {
        Some(d) => { d.nodes[id as usize].ce = CE_CUSTOM; d.touch(); d.nodes[id as usize].attrs.clone() }
        None => return,
    };
    if !matches!(i.custom.defs[k].attr_changed, Value::Undefined) {
        for (an, av) in attrs {
            if i.custom.defs[k].observed.iter().any(|o| *o == an) {
                let cb = i.custom.defs[k].attr_changed.clone();
                ce_call(i, id, &cb, "attributeChangedCallback",
                        &[Value::Str(an), Value::Null, Value::Str(av), Value::Null]);
            }
        }
    }
    if i.doc.as_ref().is_some_and(|d| d.connected(id)) {
        let cb = i.custom.defs[k].connected.clone();
        ce_call(i, id, &cb, "connectedCallback", &[]);
    }
}

/// Call one lifecycle callback. An exception is reported, not propagated:
/// the DOM operation that caused it has already happened (HTML §4.13.6).
fn ce_call(i: &mut Interp, id: u32, cb: &Value, which: &str, args: &[Value]) {
    if !i.is_callable(cb) { return }
    let this = wrap(i, id);
    if let Err(e) = i.call(cb, this, args) {
        let msg = super::modules::describe(i, e);
        let tag = i.doc.as_ref().map(|d| d.nodes[id as usize].tag.to_string()).unwrap_or_default();
        i.console_push(alloc::format!("{which} <{tag}>: {msg}"));
    }
}

/// The definition of a custom element in the "custom" state.
fn ce_def_at(i: &Interp, id: u32) -> Option<usize> {
    let d = i.doc.as_ref()?;
    let n = &d.nodes[id as usize];
    if n.ce != CE_CUSTOM { return None }
    i.custom.by_name(&n.tag)
}

/// Run the custom element reactions queued by tree changes (CEReactions).
///
/// The queue is taken whole first: a callback that changes the tree queues
/// new reactions, and the nested flush after that change runs them before
/// the callback continues, as the spec's element queue stack does.
pub fn ce_flush(i: &mut Interp) {
    let evs = match &mut i.doc {
        Some(d) if !d.ce_queue.is_empty() => core::mem::take(&mut d.ce_queue),
        _ => return,
    };
    for ev in evs {
        match ev {
            CeEvent::Inserted(n) => {
                for e in ce_subtree(i, n) {
                    match ce_def_at(i, e) {
                        Some(k) => {
                            let cb = i.custom.defs[k].connected.clone();
                            ce_call(i, e, &cb, "connectedCallback", &[]);
                        }
                        None => ce_try_upgrade(i, e),
                    }
                }
            }
            CeEvent::Created(n) => {
                if i.doc.as_ref().is_some_and(|d| ce_inert(d, n)) { continue }
                for e in ce_subtree(i, n) { ce_try_upgrade(i, e); }
            }
            CeEvent::Removed(n) => {
                for e in ce_subtree(i, n) {
                    if let Some(k) = ce_def_at(i, e) {
                        let cb = i.custom.defs[k].disconnected.clone();
                        ce_call(i, e, &cb, "disconnectedCallback", &[]);
                    }
                }
            }
            CeEvent::Attr(e, name, old, new) => {
                let Some(k) = ce_def_at(i, e) else { continue };
                if !i.custom.defs[k].observed.iter().any(|o| *o == name) { continue }
                let cb = i.custom.defs[k].attr_changed.clone();
                let v = |x: Option<Rc<str>>| x.map(Value::Str).unwrap_or(Value::Null);
                ce_call(i, e, &cb, "attributeChangedCallback",
                        &[Value::Str(name), v(old), v(new), Value::Null]);
            }
        }
    }
}

/// `document.createElement` for a defined name (HTML §4.13.3 "create an
/// element", synchronous custom elements flag set). A constructor that
/// fails is reported and yields a failed element, as in browsers.
fn ce_create_sync(i: &mut Interp, k: usize) -> C<Value> {
    let ctor = i.custom.defs[k].ctor.clone();
    let name = i.custom.defs[k].name.clone();
    let msg = match i.construct(&ctor, &[]) {
        Ok(v) => {
            let ok = match node_of_ref(i, &v) {
                Ok(id) => i.doc.as_ref().is_some_and(|d| {
                    let n = &d.nodes[id as usize];
                    n.kind == ELEMENT_NODE && *n.tag == *name && n.parent.is_none()
                        && n.children.is_empty() && n.attrs.is_empty()
                }),
                Err(()) => false,
            };
            if ok { return Ok(v) }
            String::from("NotSupportedError: the constructor did not return a fresh element")
        }
        Err(e) => super::modules::describe(i, e),
    };
    i.console_push(alloc::format!("custom element <{name}>: {msg}"));
    let Some(d) = &mut i.doc else { return i.type_err("no document") };
    let id = d.create(ELEMENT_NODE, &name);
    d.nodes[id as usize].ce = CE_FAILED;
    Ok(wrap(i, id))
}

/// Move the DOM node from one wrapper object to another.
///
/// `super()` copies the fields of the builtin result into the derived
/// class's `this`, but the node still points to the discarded wrapper, and
/// `wrap` would hand that out later. This makes the component be the
/// element rather than exist twice.
pub fn readopt(i: &mut Interp, from: &Gc, to: &Gc) {
    let id = match from.borrow().get_own(SLOT).and_then(|p| p.value.clone()) {
        Some(Value::Num(n)) => n as u32,
        _ => return,
    };
    let Some(d) = &mut i.doc else { return };
    let Some(node) = d.nodes.get_mut(id as usize) else { return };
    if matches!(&node.js, Some(g) if Rc::ptr_eq(g, from)) {
        node.js = Some(to.clone());
    }
}

/// The `<option>` nodes of a `<select>`, in document order and through
/// `<optgroup>`, exactly as `forms.rs::collect_options`.
fn select_options(i: &Interp, sel: u32) -> Vec<u32> {
    fn walk(d: &Doc, id: u32, out: &mut Vec<u32>) {
        for c in d.nodes[id as usize].children.clone() {
            if d.nodes[c as usize].kind != ELEMENT_NODE { continue }
            if &*d.nodes[c as usize].tag == "option" { out.push(c); } else { walk(d, c, out); }
        }
    }
    let mut out = Vec::new();
    if let Some(d) = &i.doc { walk(d, sel, &mut out); }
    out
}

/// An option's value: the attribute, else its text. Same rule as layout,
/// so the display and script agree.
fn option_value(i: &Interp, id: u32) -> String {
    let Some(d) = &i.doc else { return String::new() };
    match d.nodes[id as usize].attr("value") {
        Some(v) => v.to_string(),
        None => d.text_of(id).trim().to_string(),
    }
}

/// Which `<select>` does this option belong to?
fn owning_select(i: &Interp, id: u32) -> Option<u32> {
    let d = i.doc.as_ref()?;
    let mut cur = d.nodes[id as usize].parent;
    while let Some(p) = cur {
        if &*d.nodes[p as usize].tag == "select" { return Some(p) }
        cur = d.nodes[p as usize].parent;
    }
    None
}

/// Which option is selected?
///
/// If none has `selected`, a single-select picks the first, as browsers
/// display it.
fn selected_index(i: &Interp, sel: u32) -> f64 {
    let opts = select_options(i, sel);
    let Some(d) = &i.doc else { return -1.0 };
    for (n, o) in opts.iter().enumerate() {
        if d.nodes[*o as usize].attr("selected").is_some() { return n as f64 }
    }
    let multiple = d.nodes[sel as usize].attr("multiple").is_some();
    if !multiple && !opts.is_empty() { 0.0 } else { -1.0 }
}

/// Select the n-th option and deselect all others. `-1` selects none.
fn select_index(i: &mut Interp, sel: u32, n: i64) {
    let opts = select_options(i, sel);
    let Some(d) = &mut i.doc else { return };
    d.touch();
    for (k, o) in opts.iter().enumerate() {
        if k as i64 == n { d.set_attr_at(*o, "selected", ""); }
        else { d.remove_attr_at(*o, "selected"); }
    }
}

/// Replace a node's content with one text node; the `textContent` rule,
/// as a function because `option.text` needs it too.
fn set_text_of(i: &mut Interp, id: u32, s: &str) {
    let Some(d) = &mut i.doc else { return };
    d.clear_children(id);
    if !s.is_empty() {
        let tid = d.create(TEXT_NODE, "#text");
        d.nodes[tid as usize].text = s.into();
        d.append(id, tid);
    }
}

/// Is this node attached to the document, through shadow roots?
fn is_connected(d: &Doc, id: u32) -> bool { d.connected(id) }

/// `document.write`/`writeln`: insert the fragment after the writing
/// `<script>` and connect everything in it.
///
/// Connecting also means fetching: a written `<script src>` runs, unlike
/// one from `innerHTML`.
fn doc_write(i: &mut Interp, this: &Value, a: &[Value], line: bool) -> C<Value> {
    let root = node_of(i, this)?;
    if i.doc.as_ref().map(|d| d.doc) != Some(root) { return Ok(Value::Undefined) }
    let mut html = String::new();
    for v in a { html.push_str(&i.to_string(v)?); }
    if line { html.push('\n'); }
    if html.trim().is_empty() { return Ok(Value::Undefined) }
    let Some(script) = i.current_script else {
        i.console_push(alloc::format!(
            "document.write ohne laufendes Skript — nichts geschrieben ({} B)", html.len()));
        return Ok(Value::Undefined);
    };
    // The insertion point: after the last thing this script wrote, else after
    // the script itself.
    let after = match i.write_point {
        Some((s, last)) if s == script => last,
        _ => script,
    };
    let Some(d) = &mut i.doc else { return Ok(Value::Undefined) };
    let Some(parent) = d.nodes[after as usize].parent else { return Ok(Value::Undefined) };
    let at = d.nodes[parent as usize].children.iter().position(|&c| c == after).map(|k| k + 1);
    let made = d.parse_into(parent, &html, at);
    if let Some(&last) = made.last() { i.write_point = Some((script, last)); }
    // Nested ones too: a `<script>` inside a written `<div>` must run as
    // well.
    for n in made {
        let mut all = alloc::vec![n];
        if let Some(d) = &i.doc { d.descendants(n, &mut all); }
        for x in all { fire_connected(i, x)?; }
    }
    Ok(Value::Undefined)
}

/// Settle what just entered the tree: stylesheets and scripts start loading,
/// custom element reactions run.
fn fire_connected(i: &mut Interp, id: u32) -> C<Value> {
    settle_stylesheet(i, id);
    settle_script(i, id);
    ce_flush(i);
    Ok(Value::Undefined)
}

/// A code point as lowercase hex digits, the form `CSS.escape` requires.
fn push_hex(out: &mut String, mut code: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 8];
    let mut n = 0;
    if code == 0 { buf[0] = b'0'; n = 1; }
    while code > 0 { buf[n] = HEX[(code & 0xF) as usize]; code >>= 4; n += 1; }
    for k in (0..n).rev() { out.push(buf[k] as char); }
}

/// A `<link rel="stylesheet">` inserted by script is queued for fetching.
///
/// The engine fetches nothing: it puts the URL into `pending_sheets`, the
/// host loads it and reports back with `sheet_done`. Only then does `load`
/// or `error` fire on the `<link>`. Pages that load stylesheets from script
/// usually wait for that `load`.
fn settle_stylesheet(i: &mut Interp, id: u32) {
    let is_sheet = i.doc.as_ref().is_some_and(|d| {
        let n = &d.nodes[id as usize];
        &*n.tag == "link"
            && n.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet"))
            && n.attr("href").is_some_and(|h| !h.trim().is_empty())
            && is_connected(d, id)
    });
    if !is_sheet { return }
    // Queuing twice would fetch twice and fire `load` twice.
    if i.pending_sheets.iter().any(|(n, _)| *n == id) { return }
    let href = i.doc.as_ref().and_then(|d| d.nodes[id as usize].attr("href").cloned())
        .unwrap_or_else(|| Rc::from(""));
    i.pending_sheets.push((id, href.to_string()));
}

/// The host reports how a requested stylesheet fared.
pub fn sheet_done(i: &mut Interp, id: u32, ok: bool) {
    let _ = dispatch(i, if ok { "load" } else { "error" }, &[id]);
}

/// A `<script src>` inserted by script is queued for fetching. This is how
/// bundlers load their chunks: insert a `<script>` and wait for `onload`.
///
/// Limits:
/// * Only with `src`. An inserted script with inline text runs immediately
///   per spec; not handled here.
/// * Only once: the spec's "already started" flag; reinserting does not
///   run it again.
/// * No modules. `type="module"` needs the module graph, which the host
///   resolves at page load; not implemented here.
fn settle_script(i: &mut Interp, id: u32) {
    let src = i.doc.as_ref().and_then(|d| {
        let n = &d.nodes[id as usize];
        if &*n.tag != "script" || !is_connected(d, id) { return None }
        if n.attr("type").is_some_and(|t| {
            let t = t.trim().to_ascii_lowercase();
            !(t.is_empty() || t.contains("javascript") || t == "text/ecmascript")
        }) { return None }
        n.attr("src").filter(|s| !s.trim().is_empty()).cloned()
    });
    let Some(src) = src else { return };
    if i.ran_scripts.contains(&id) { return }
    if i.pending_scripts.iter().any(|(n, _)| *n == id) { return }
    i.ran_scripts.push(id);
    i.pending_scripts.push((id, src.to_string()));
}

/// The host reports what became of a requested script.
///
/// `Some(source)` means fetched: the text runs in the page's scope, and
/// then `load` fires, the browser order loaders rely on. An exception
/// ends only this script; `load` still fires.
pub fn script_done(i: &mut Interp, id: u32, source: Option<&str>) {
    let Some(src) = source else {
        let _ = dispatch(i, "error", &[id]);
        return;
    };
    match super::parse(src, false) {
        Ok(prog) => {
            // `document.currentScript` points to this node while it runs, then back
            // to the current one: an inserted `<script>` runs from inside another.
            let outer = i.current_script.replace(id);
            let r = i.run_program(&prog);
            i.current_script = outer;
            if let Err(a) = r {
                let msg = super::modules::describe(i, a);
                i.console_push(alloc::format!("error: {msg}"));
            }
        }
        Err(e) => i.console_push(alloc::format!("error: SyntaxError: {} @{}", e.msg, e.at)),
    }
    let _ = dispatch(i, "load", &[id]);
}

/// Report an unhandled rejection to the window. Returns true if a handler
/// called `preventDefault`; then the console line is skipped.
pub fn dispatch_rejection(i: &mut Interp, reason: Value, promise: Value) -> C<bool> {
    let Some(target) = i.doc.as_ref().map(|d| d.doc) else { return Ok(false) };
    let proto = i.realm.prej_proto.clone();
    let ev = build_event(i, proto, "unhandledrejection", true);
    ev.borrow_mut().define(EV_REASON, Prop::data(reason));
    ev.borrow_mut().define(EV_PROMISE, Prop::data(promise));
    deliver(i, &ev, "unhandledrejection", &[target])
}

/// Dispatch `focus`/`blur`. They do not bubble: the path is the element
/// alone; `focusin`/`focusout` are the bubbling twins.
fn deliver_focus(i: &mut Interp, id: u32, kind: &str) -> C<()> {
    dispatch(i, kind, &[id])?;
    Ok(())
}

/// The value of a control: the dirty one, else the default.
///
/// For `<textarea>` the default is the text content, for `<input>` the
/// `value` attribute.
fn control_value(i: &Interp, id: u32, from_text: bool) -> String {
    let Some(d) = &i.doc else { return String::new() };
    if let Some(v) = &d.nodes[id as usize].value { return v.to_string() }
    if from_text { return d.text_of(id) }
    d.nodes[id as usize].attr("value").map(|v| v.to_string()).unwrap_or_default()
}

fn set_control_value(i: &mut Interp, id: u32, v: &str) {
    if let Some(d) = &mut i.doc {
        d.nodes[id as usize].value = Some(Rc::from(v));
        d.touch();
    }
}

/// The checkbox state as it stands now: the dirty value, else the
/// attribute.
fn checked_now(i: &Interp, id: u32) -> bool {
    let Some(d) = i.doc.as_ref() else { return false };
    match d.nodes.get(id as usize).and_then(|n| n.checked) {
        Some(b) => b,
        None => d.nodes.get(id as usize).is_some_and(|n| n.attr("checked").is_some()),
    }
}

fn set_checked(i: &mut Interp, id: u32, on: bool) {
    if let Some(d) = &mut i.doc {
        if let Some(n) = d.nodes.get_mut(id as usize) {
            n.checked = Some(on);
        }
        d.touch();
    }
}

/// The form a control belongs to: the nearest `<form>` ancestor. Not
/// implemented: the `form=` attribute (same limit as `forms::collect`).
fn owning_form(i: &Interp, id: u32) -> Option<u32> {
    let d = i.doc.as_ref()?;
    let mut cur = d.nodes.get(id as usize)?.parent;
    while let Some(p) = cur {
        if &*d.nodes.get(p as usize)?.tag == "form" {
            return Some(p);
        }
        cur = d.nodes[p as usize].parent;
    }
    None
}

/// The `<label>`s of a control: those wrapping it and those pointing at
/// its `id` via `for=`.
fn labels_of(i: &Interp, id: u32) -> Vec<u32> {
    let Some(d) = i.doc.as_ref() else { return Vec::new() };
    let mut out = Vec::new();
    let mut cur = d.nodes.get(id as usize).and_then(|n| n.parent);
    while let Some(p) = cur {
        let Some(n) = d.nodes.get(p as usize) else { break };
        if &*n.tag == "label" && !out.contains(&p) {
            out.push(p);
        }
        cur = n.parent;
    }
    if let Some(want) = d.nodes.get(id as usize).and_then(|n| n.attr("id")).cloned() {
        for (k, n) in d.nodes.iter().enumerate() {
            if &*n.tag == "label" && n.attr("for").is_some_and(|f| **f == *want) && !out.contains(&(k as u32)) {
                out.push(k as u32);
            }
        }
    }
    out
}

/// The control a `<label>` names: `for=` first, else the first wrapped
/// one.
fn label_control(i: &Interp, id: u32) -> Option<u32> {
    let d = i.doc.as_ref()?;
    if let Some(want) = d.nodes.get(id as usize)?.attr("for").cloned() {
        return d.nodes.iter().position(|n| {
            n.kind == ELEMENT_NODE && n.attr("id").is_some_and(|v| **v == *want)
        }).map(|k| k as u32);
    }
    fn first(d: &Doc, id: u32, out: &mut Option<u32>) {
        if out.is_some() { return }
        for c in &d.nodes[id as usize].children {
            let t = &*d.nodes[*c as usize].tag;
            if matches!(t, "input" | "select" | "textarea" | "button") {
                *out = Some(*c);
                return;
            }
            first(d, *c, out);
        }
    }
    let mut out = None;
    first(d, id, &mut out);
    out
}

fn t_is_textarea(i: &Interp, id: u32) -> bool {
    i.doc.as_ref().is_some_and(|d| d.nodes.get(id as usize).is_some_and(|n| &*n.tag == "textarea"))
}

/// The controls of a `<form>`, in document order. Found through the tree;
/// the `form=` attribute is not implemented.
fn form_controls(i: &Interp, form: u32) -> Vec<u32> {
    fn walk(d: &Doc, id: u32, out: &mut Vec<u32>) {
        for c in d.nodes[id as usize].children.clone() {
            if d.nodes[c as usize].kind != ELEMENT_NODE { continue }
            let tag = &*d.nodes[c as usize].tag;
            if matches!(tag, "input" | "select" | "textarea" | "button" | "fieldset" | "output") {
                out.push(c);
            }
            // A nested `<form>` is invalid HTML; its elements belong to it, not to
            // us.
            if tag != "form" { walk(d, c, out); }
        }
    }
    let mut out = Vec::new();
    if let Some(d) = &i.doc { walk(d, form, &mut out); }
    out
}

/// All elements with a tag in a subtree, in document order.
fn tags_of(d: &Doc, from: u32, tag: &str) -> Vec<u32> {
    fn walk(d: &Doc, id: u32, tag: &str, out: &mut Vec<u32>) {
        for c in d.nodes[id as usize].children.clone() {
            if d.nodes[c as usize].kind != ELEMENT_NODE { continue }
            if &*d.nodes[c as usize].tag == tag { out.push(c); }
            walk(d, c, tag, out);
        }
    }
    let mut out = Vec::new();
    walk(d, from, tag, &mut out);
    out
}

/// The prototype of an event kind, via its global name.
fn iface_proto(i: &mut Interp, iface: &str) -> Gc {
    match i.get(&Value::Obj(i.realm.global.clone()), iface)
           .and_then(|c| i.get(&c, "prototype")) {
        Ok(Value::Obj(o)) => o,
        _ => i.realm.event_proto.clone(),
    }
}

/// Dispatch an event of a given kind at the tree node `seq`.
///
/// The single path for everything the host sends. Returns `true` if the
/// page canceled (`preventDefault`), and the caller must honour it: a
/// canceled `keydown` must not insert a character.
fn dispatch_typed(i: &mut Interp, iface: &str, kind: &str, seq: u32,
                  bubbles: bool, cancelable: bool,
                  fields: &[(&str, Value)]) -> bool {
    let Some(doc) = i.doc.as_ref() else { return false };
    let Some(id) = doc.by_seq(seq) else { return false };
    let chain = ancestors(i, id);
    if chain.is_empty() { return false }
    let proto = iface_proto(i, iface);
    let ev = build_event(i, proto, kind, true);
    {
        let mut o = ev.borrow_mut();
        let mut put = |k: &str, v: Value| {
            o.define(k, Prop { value: Some(v), get: None, set: None,
                writable: true, enumerable: false, configurable: true });
        };
        put(EV_BUBBLES, Value::Bool(bubbles));
        put(EV_CANCELABLE, Value::Bool(cancelable));
        for (k, v) in fields { put(k, v.clone()); }
    }
    matches!(deliver(i, &ev, kind, &chain), Ok(true))
}

/// `keydown`/`keyup` at the control with this `seq`.
///
/// `key` is the key's value (`"a"`, `"Enter"`, `"ArrowLeft"`), `code` its
/// location (`"KeyA"`). `key_code` is the legacy field everyone still
/// reads. Returns `true` if the page canceled.
pub fn dispatch_key(i: &mut Interp, kind: &str, seq: u32, key: &str, code: &str,
                    key_code: u32, shift: bool) -> bool {
    dispatch_typed(i, "KeyboardEvent", kind, seq, true, true, &[
        ("__evkey", Value::str(key)),
        ("__evcode", Value::str(code)),
        ("__evkeycode", Value::Num(key_code as f64)),
        ("__evcharcode", Value::Num(0.0)),
        ("__evlocation", Value::Num(0.0)),
        ("__evrepeat", Value::Bool(false)),
        ("__evcomposing", Value::Bool(false)),
        ("__evalt", Value::Bool(false)),
        ("__evctrl", Value::Bool(false)),
        ("__evshift", Value::Bool(shift)),
        ("__evmeta", Value::Bool(false)),
        (EV_DETAIL, Value::Num(0.0)),
    ])
}

/// `beforeinput`/`input` at the control with this `seq`.
///
/// `beforeinput` is cancelable, `input` is not (UI Events §5.1).
pub fn dispatch_input_event(i: &mut Interp, kind: &str, seq: u32,
                            input_type: &str, data: Option<&str>) -> bool {
    let d = match data { Some(t) => Value::str(t), None => Value::Null };
    dispatch_typed(i, "InputEvent", kind, seq, true, kind == "beforeinput", &[
        ("__evdata", d),
        ("__evinputtype", Value::str(input_type)),
        ("__evcomposing", Value::Bool(false)),
        (EV_DETAIL, Value::Num(0.0)),
    ])
}

/// `focus`/`blur` (do not bubble) and `focusin`/`focusout` (bubble).
///
/// Both pairs, because `focus` reaches only the element itself while
/// `focusin` travels up; a page listening on the form hears only the
/// latter.
pub fn dispatch_focus(i: &mut Interp, kind: &str, seq: u32, related: Option<u32>) -> bool {
    let rel = match related.and_then(|r| i.doc.as_ref().and_then(|d| d.by_seq(r))) {
        Some(id) => wrap(i, id),
        None => Value::Null,
    };
    let bubbles = kind == "focusin" || kind == "focusout";
    dispatch_typed(i, "FocusEvent", kind, seq, bubbles, false, &[
        ("__evrelated", rel),
        (EV_DETAIL, Value::Num(0.0)),
    ])
}

/// Dispatch an event at the element with this `seq`, bubbling up to the
/// root. The path for events the host itself fires (a form submission, an
/// element gaining focus). Returns true if a handler called
/// `preventDefault` (or returned `false`).
pub fn dispatch_seq(i: &mut Interp, kind: &str, seq: u32) -> bool {
    let Some(doc) = i.doc.as_ref() else { return false };
    let Some(id) = doc.by_seq(seq) else { return false };
    let mut chain = alloc::vec![id];
    let mut cur = doc.nodes[id as usize].parent;
    while let Some(p) = cur {
        chain.push(p);
        cur = doc.nodes[p as usize].parent;
    }
    chain.reverse();
    matches!(dispatch(i, kind, &chain), Ok(true))
}

// ── Bridge between user input and the tree ───────────────────────────────
//
// Two stores, one rule. User input lives in the host (`FormState`, keyed
// by `seq`), because a page without scripts has no engine tree; the dirty
// value lives on the node, because `el.value` expects it there. They are
// synced before and after every run of page code, through these two
// functions only.

/// Write user input into the tree before any page code runs. Only edited
/// fields are transferred; otherwise every field would carry a dirty value
/// and `form.reset()` would have nothing to restore.
pub fn push_control_values(doc: &mut Doc, forms: &crate::forms::Forms,
                           state: &crate::forms::FormState) {
    use crate::forms::ControlKind;
    for c in &forms.controls {
        let Some(id) = doc.by_seq(c.seq) else { continue };
        match c.kind {
            ControlKind::Checkbox | ControlKind::Radio => {
                if let Some(b) = state.checked_set(c.seq) { doc.nodes[id as usize].checked = Some(b); }
            }
            _ => {
                if let Some(v) = state.value_set(c.seq) {
                    doc.nodes[id as usize].value = Some(Rc::from(v));
                }
            }
        }
    }
}

/// And back: what page code set applies to display and submission.
pub fn pull_control_values(doc: &Doc, forms: &crate::forms::Forms,
                           state: &mut crate::forms::FormState) {
    let mut set: Vec<(u32, Option<Rc<str>>, Option<bool>)> = Vec::new();
    for c in &forms.controls {
        let Some(id) = doc.by_seq(c.seq) else { continue };
        let n = &doc.nodes[id as usize];
        if n.value.is_none() && n.checked.is_none() { continue }
        set.push((c.seq, n.value.clone(), n.checked));
    }
    for (seq, v, ch) in set {
        if let Some(v) = v { state.set_value(seq, v.to_string()); }
        if let Some(b) = ch { state.set_checked(seq, b); }
    }
}

#[cfg(test)]
mod ce_tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Run on both engines; the console must agree.
    pub(super) fn run(html: &str, js: &str) -> Vec<String> {
        let mut outs = Vec::new();
        for vm_off in [false, true] {
            let dom = crate::dom::parse(html);
            let mut i = super::super::interp::Interp::new();
            i.vm_off = vm_off;
            i.set_document(super::Doc::from_dom(&dom));
            let prog = super::super::parse(js, false).expect("parses");
            if let Err(e) = i.run_program(&prog) {
                let m = super::super::modules::describe(&mut i, e);
                i.console_push(alloc::format!("THREW {m}"));
            }
            super::super::promise::run_jobs(&mut i);
            outs.push(i.take_console());
        }
        assert_eq!(outs[0], outs[1], "tree walker and VM disagree");
        outs.pop().unwrap()
    }

    #[test]
    fn define_upgrades_existing_elements_in_document_order() {
        let out = run(
            "<body><x-a id=one k=v></x-a><div><x-a id=two></x-a></div><template><x-a id=t></x-a></template></body>",
            "var held = document.getElementById('one');\
             class A extends HTMLElement {\
               static get observedAttributes() { return ['k']; }\
               constructor() { super(); console.log('ctor ' + this.id); }\
               connectedCallback() { console.log('conn ' + this.id); }\
               attributeChangedCallback(n, o, v) { console.log('attr ' + n + ' ' + o + ' ' + v); }\
               hi() { return 'hi ' + this.id; }\
             }\
             customElements.define('x-a', A);\
             console.log(held instanceof A, held === document.getElementById('one'), held.hi());\
             console.log(document.querySelector('template').innerHTML.length > 0);",
        );
        assert_eq!(out, ["ctor one", "attr k null v", "conn one", "ctor two", "conn two",
                         "true true hi one", "true"]);
    }

    #[test]
    fn create_element_constructs_synchronously_and_insertion_connects() {
        let out = run(
            "<body><main></main></body>",
            "class B extends HTMLElement {\
               static observedAttributes = ['x'];\
               connectedCallback() { console.log('conn', this.isConnected); }\
               disconnectedCallback() { console.log('disc'); }\
               attributeChangedCallback(n, o, v) { console.log('attr', n, o, v); }\
             }\
             customElements.define('x-b', B);\
             var e = document.createElement('x-b');\
             console.log(e instanceof B, e.localName, customElements.get('x-b') === B, customElements.getName(B));\
             e.setAttribute('x', '1'); e.setAttribute('y', '2'); e.removeAttribute('x');\
             var m = document.querySelector('main');\
             m.appendChild(e); console.log('after append');\
             e.remove(); console.log('after remove');\
             m.innerHTML = '<x-b x=7></x-b>';\
             console.log(m.firstChild instanceof B);\
             var n = new B(); console.log(n.tagName);",
        );
        assert_eq!(out, ["true x-b true x-b", "attr x null 1", "attr x 1 null",
                         "conn true", "after append", "disc", "after remove",
                         "attr x null 7", "conn true", "true", "X-B"]);
    }

    #[test]
    fn reflect_construct_and_when_defined() {
        let out = run(
            "<body><x-c></x-c></body>",
            "function C() { return Reflect.construct(HTMLElement, [], C); }\
             C.prototype = Object.create(HTMLElement.prototype);\
             C.prototype.constructor = C;\
             Object.setPrototypeOf(C, HTMLElement);\
             C.prototype.connectedCallback = function() { console.log('conn c'); };\
             customElements.whenDefined('x-c').then(function(k) { console.log('defined', k === C); });\
             customElements.define('x-c', C);\
             console.log(document.querySelector('x-c') instanceof C);\
             try { customElements.define('x-c', class extends HTMLElement {}); }\
             catch (e) { console.log(e.name, e instanceof DOMException); }\
             try { customElements.define('nohyphen', class extends HTMLElement {}); }\
             catch (e) { console.log(e.name); }\
             try { new HTMLElement(); } catch (e) { console.log(e.constructor.name); }",
        );
        assert_eq!(out, ["conn c", "true", "NotSupportedError true", "SyntaxError",
                         "TypeError", "defined true"]);
    }

    #[test]
    fn a_failing_constructor_leaves_a_failed_element() {
        let out = run(
            "<body><x-d></x-d></body>",
            "customElements.define('x-d', class extends HTMLElement {\
               constructor() { super(); throw new Error('boom'); } });\
             document.body.appendChild(document.createElement('x-d'));\
             console.log('still running');",
        );
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out[0].contains("upgrade failed") && out[0].contains("boom"), "{out:?}");
        assert!(out[1].contains("boom"), "{out:?}");
        assert_eq!(out[2], "still running");
    }

    #[test]
    fn nested_insertions_in_callbacks_run_before_the_callback_continues() {
        let out = run(
            "<body></body>",
            "customElements.define('x-in', class extends HTMLElement {\
               connectedCallback() { console.log('inner'); } });\
             customElements.define('x-out', class extends HTMLElement {\
               connectedCallback() { this.appendChild(document.createElement('x-in'));\
                                     console.log('outer done'); } });\
             document.body.appendChild(document.createElement('x-out'));",
        );
        assert_eq!(out, ["inner", "outer done"]);
    }
}
