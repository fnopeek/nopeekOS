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
use hashbrown::HashMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

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
pub const COMMENT_NODE: f64 = 8.0;
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
    /// Angemeldete Behandler, je Ereignisart.
    pub listeners: Vec<(Rc<str>, Value)>,
    /// Behandler, die als EIGENSCHAFT gesetzt wurden (`el.onclick = f`).
    ///
    /// Getrennt von `listeners`, weil sie sich anders verhalten: eine zweite
    /// Zuweisung ERSETZT die erste, waehrend `addEventListener` anhaengt. Und
    /// sie verdraengt das gleichnamige Attribut — im Browser ist es derselbe
    /// Platz, nicht zwei.
    pub handlers: Vec<(Rc<str>, Value)>,
    /// Der „schmutzige" Wert eines Steuerelements (HTML §4.10.5.5): was der
    /// Benutzer getippt oder ein Skript gesetzt hat.
    ///
    /// **Getrennt vom Attribut, und das ist keine Feinheit.** `el.value = x`
    /// aendert im Browser das `value`-ATTRIBUT NICHT — das bleibt der
    /// Vorgabewert (`defaultValue`), auf den `form.reset()` zurueckstellt.
    /// Wer beides zusammenlegt, hat eine Seite, die nach dem Zuruecksetzen
    /// das Getippte wieder hinschreibt, und `getAttribute("value")` luegt.
    pub value: Option<Rc<str>>,
    /// Dasselbe fuer `checked`: `defaultChecked` ist das Attribut.
    pub checked: Option<bool>,
    /// Nur `<template>`: der Bruchstueck-Knoten, in dem sein Inhalt haengt.
    /// Entsteht beim ersten `.content` — siehe dort, warum nicht frueher.
    pub content: Option<u32>,
    /// Die `seq` desselben Elements in beaks Baum — die Bruecke zwischen
    /// Klickpunkt und Knoten. `to_dom` vergibt sie und schreibt sie HIER
    /// zurueck; das Layout gibt sie beim Treffer aus. Ohne diese Brueckenzahl
    /// gibt es keinen Weg von „hier wurde geklickt" zu „dieser Knoten".
    pub seq: u32,
    /// Die `seq` des Elements im BAUM, aus dem dieses Dokument gebaut wurde.
    ///
    /// Die Bruecke fuer `getComputedStyle`: die Kaskade laeuft auf beaks Baum
    /// (`crate::dom`), die Maschine arbeitet auf ihrer eigenen Arena. Ohne
    /// diesen Verweis gibt es keinen Weg von „dieses JS-Objekt" zu „dieses
    /// Element, fuer das die Kaskade gerechnet hat".
    ///
    /// `0` heisst „kein Quellknoten" — ein Element, das ein Skript erst
    /// erzeugt hat. Fuer das kann `getComputedStyle` nur den Inline-Stil
    /// beantworten, und das ist ehrlicher als eine Zahl aus dem Nichts.
    pub src_seq: u32,
}

impl DomNode {
    fn new(kind: f64, tag: &str) -> DomNode {
        DomNode { kind, tag: Rc::from(tag), attrs: Vec::new(), text: Rc::from(""),
                  parent: None, children: Vec::new(), js: None, listeners: Vec::new(),
                  handlers: Vec::new(), content: None, value: None, checked: None,
                  seq: 0, src_seq: 0 }
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

/// Was sich am Baum geaendert hat — die Rohaufzeichnung fuer
/// `MutationObserver`.
///
/// **Aufgezeichnet wird im BAUM, nicht in der Bindung.** Ein Beobachter, der
/// sich seine Meldungen aus einem Vergleich zweier Momentaufnahmen rechnet,
/// meldet „geaendert" und weiss nicht, was; und er kann nicht sehen, dass ein
/// Knoten weg war und wieder da ist. Die Stellen, an denen sich ein Baum
/// aendert, sind gezaehlt — also sagen sie es selbst.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MutKind { ChildList, Attributes, CharacterData }

#[derive(Clone)]
pub struct Mutation {
    pub kind: MutKind,
    /// Bei `ChildList` der ELTER, bei den anderen beiden der Knoten selbst.
    pub target: u32,
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    pub prev: Option<u32>,
    pub next: Option<u32>,
    pub attr: Option<Rc<str>>,
    pub old: Option<Rc<str>>,
    /// **Welche beobachteten Knoten diese Aenderung ZUM ZEITPUNKT DER
    /// AENDERUNG umschlossen haben**, als Bitmaske ueber `Doc::observed`.
    ///
    /// Das ist der ganze Grund, warum es diese Maske gibt: die Zugehoerigkeit
    /// spaeter zu pruefen liest den Baum von SPAETER. Ein Knoten, der erst
    /// gesetzt und dann eingehaengt wird, waere faelschlich dabei; einer, der
    /// geaendert und dann entfernt wird, fiele heraus. Beides ist mir genau
    /// so passiert, und beides sieht aus wie ein Fehler der Bibliothek.
    pub hits: u64,
}

/// Deckel fuer die Rohaufzeichnung.
///
/// Ein Skript, das hunderttausend Knoten baut, waehrend ein Beobachter
/// angemeldet ist, darf den Speicher nicht auffressen. Ueberlaeuft es, wird
/// das GEMELDET und nicht still verschluckt — ein Beobachter, dem Meldungen
/// fehlen, ohne dass es jemand sagt, ist schlimmer als gar keiner.
pub const MAX_MUTATIONS: usize = 20_000;

pub struct Doc {
    pub nodes: Vec<DomNode>,
    pub doc: u32,
    pub html: Option<u32>,
    pub body: Option<u32>,
    pub head: Option<u32>,
    /// Hat sich seit dem letzten Zurueckschreiben etwas geaendert?
    ///
    /// Ohne diese Fahne muesste JEDER Klick den Baum neu aufbauen und die
    /// Seite neu auslegen — 130 ms auf dem Geraet, fuer nichts, wenn der
    /// Behandler nur etwas gezaehlt hat.
    pub dirty: bool,
    /// Hat die Seite ueberhaupt Behandler? Solange nicht, braucht das Layout
    /// keine Treffer-Kaesten aufzuzeichnen.
    pub has_listeners: bool,
    /// Welches Element den Tastaturfokus hat — gesetzt von `focus()`.
    ///
    /// Der Wirt liest es und stellt seinen eigenen Fokus danach
    /// (`forms.rs::FormState::focus`); ohne diese Zeile war `el.focus()`
    /// „focus is not a function", und die Fritzbox-Anmeldemaske ist genau
    /// daran am Ende ihres Aufbaus gescheitert.
    pub focused: Option<u32>,
    /// Zaehlt jede Aenderung am Baum — und wird NIE zurueckgesetzt.
    ///
    /// `dirty` beantwortet „muss ich zurueckschreiben?" und wird beim
    /// Zurueckschreiben geloescht. Fuer einen Zwischenspeicher taugt es
    /// deshalb nicht: nach dem Loeschen ist „unveraendert seit meinem Stand"
    /// von „seither zweimal geaendert" nicht mehr zu unterscheiden. Ein
    /// Zaehler, der nur steigt, kann beides.
    pub version: u32,
    /// Die Rohaufzeichnung fuer `MutationObserver` — leer, solange niemand
    /// zusieht.
    pub mutations: Vec<Mutation>,
    /// Sieht ueberhaupt jemand zu? Ohne diese Fahne zahlte JEDE Seite fuer
    /// eine Aufzeichnung, die keiner abholt.
    pub observing: bool,
    /// Ist die Aufzeichnung uebergelaufen? Wird beim Zustellen gemeldet.
    pub mut_overflow: bool,
    /// Die beobachteten Knoten als `(Knoten, mit Nachkommen)`, in der
    /// Reihenfolge der Bits in `Mutation::hits`. Der Baum muss NICHT wissen,
    /// wer zusieht — nur, wo.
    pub observed: Vec<(u32, bool)>,
}

impl Doc {
    pub fn empty() -> Doc {
        let mut nodes = Vec::new();
        nodes.push(DomNode::new(DOCUMENT_NODE, "#document"));
        Doc { nodes, doc: 0, html: None, body: None, head: None,
              dirty: false, has_listeners: false, focused: None, version: 0,
              mutations: Vec::new(), observing: false, mut_overflow: false,
              observed: Vec::new() }
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
        // Der Aufbau selbst ist keine Aenderung.
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
                        // Ein Attribut-Behandler ist genauso ein Behandler wie
                        // ein angemeldeter: ohne diese Zeile bekaeme die Seite
                        // keine Treffer-Kaesten, und der Klick fiele ins Leere.
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

    /// Frei stehender Knoten, noch ohne Elternteil (`createElement`).
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

    /// Welche beobachteten Knoten `node` JETZT umschliessen.
    ///
    /// Ein Gang die Elternkette hinauf, genau wie ihn ein echter Motor an
    /// dieser Stelle macht — die Kosten sind die Tiefe, nicht die Zahl der
    /// Knoten.
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

    /// Eine Aenderung notieren — nur, wenn jemand zusieht, und nur, wenn sie
    /// ueberhaupt jemanden betrifft.
    pub fn record(&mut self, mut m: Mutation) {
        if !self.observing { return }
        m.hits = self.hits_for(m.target);
        if m.hits == 0 { return }
        if self.mutations.len() >= MAX_MUTATIONS { self.mut_overflow = true; return }
        self.mutations.push(m);
    }

    /// Die Geschwister eines Knotens, wie sie JETZT stehen. Ein Datensatz
    /// nennt sie so, wie sie zum Zeitpunkt der Aenderung waren — danach
    /// stimmen sie nicht mehr.
    fn siblings(&self, parent: u32, at: usize) -> (Option<u32>, Option<u32>) {
        let k = &self.nodes[parent as usize].children;
        (if at > 0 { k.get(at - 1).copied() } else { None }, k.get(at + 1).copied())
    }

    /// Aus dem alten Elternteil aushaengen. Muss VOR jedem Einhaengen laufen,
    /// sonst steht ein Knoten in zwei Kinderlisten und der Baum ist keiner mehr.
    pub fn detach(&mut self, id: u32) {
        self.touch();
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
        // **Ein Umhaengen ist ein Entfernen UND ein Einfuegen**, und beides
        // gehoert gemeldet — die Spezifikation sagt es so, und ein
        // Beobachter, der nur das Einfuegen saehe, haette den Knoten
        // zweimal im Baum.
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
    }

    /// Einhaengen — und ein BRUCHSTUECK gibt dabei seine Kinder ab.
    ///
    /// Das ist keine Feinheit, sondern der Sinn der Sache: `ul.appendChild(
    /// tpl.content.cloneNode(true))` ist die uebliche Schreibweise, und wer
    /// das Bruchstueck selbst einhaengt, bekommt ein `<#fragment>`-Element in
    /// den Baum — ein Element, das es im HTML nicht gibt und das jede
    /// Formatierung darunter verschiebt.
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
    }

    /// Ein Attribut setzen — **der eine Weg**, damit die Aenderung EINMAL
    /// notiert wird und nicht an zwoelf Stellen vergessen.
    pub fn set_attr_at(&mut self, id: u32, k: &str, v: &str) {
        self.touch();
        if self.observing {
            let old = self.nodes[id as usize].attr(k).cloned();
            self.record(Mutation { kind: MutKind::Attributes, target: id,
                added: Vec::new(), removed: Vec::new(), prev: None, next: None,
                attr: Some(Rc::from(k)), old, hits: 0 });
        }
        self.nodes[id as usize].set_attr(k, v);
    }

    /// Ein Attribut entfernen. Ist es gar nicht da, ist es KEINE Aenderung —
    /// ein Datensatz dafuer waere eine erfundene Meldung.
    pub fn remove_attr_at(&mut self, id: u32, k: &str) {
        self.touch();
        let old = self.nodes[id as usize].attr(k).cloned();
        if old.is_none() { return }
        if self.observing {
            self.record(Mutation { kind: MutKind::Attributes, target: id,
                added: Vec::new(), removed: Vec::new(), prev: None, next: None,
                attr: Some(Rc::from(k)), old, hits: 0 });
        }
        self.nodes[id as usize].attrs.retain(|(n, _)| &**n != k);
    }

    /// Den Text eines Text-/Kommentarknotens setzen (`data`, `nodeValue`).
    /// NICHT fuer frisch gebaute Knoten: dort gibt es keinen alten Wert und
    /// niemanden, der zusieht.
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
    /// HTML in einen Knoten parsen — als BRUCHSTUECK, nicht als Dokument.
    ///
    /// `dom::parse` baut immer ein ganzes Dokument: `<li>x</li>` wird zu
    /// `<html><body><li>x`. Wer das ungefiltert einhaengt, schiebt bei JEDEM
    /// `innerHTML` ein `<html><body>` unter das Element — gefunden beim
    /// Bauen von `append`, und es betraf jede Seite, die `innerHTML` setzt.
    /// Sichtbar war es nicht, weil `<html>` und `<body>` keinen eigenen
    /// Kasten malen; kaputt war es trotzdem: `el.children[0]` war nicht das
    /// erste Element des Textes, sondern der Rahmen darum.
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
        self.touch();
        let old: Vec<u32> = self.nodes[id as usize].children.clone();
        for c in &old { self.nodes[*c as usize].parent = None; }
        self.nodes[id as usize].children.clear();
        // `innerHTML = "…"` raeumt hier ab, bevor es neu baut. Ohne diese
        // Meldung saehe ein Beobachter nur das Neue und nie, dass das Alte
        // weg ist — und genau daran haengen die Aufraeumroutinen jeder
        // Komponentenbibliothek.
        if self.observing && !old.is_empty() {
            self.record(Mutation { kind: MutKind::ChildList, target: id,
                added: Vec::new(), removed: old, prev: None, next: None,
                attr: None, old: None, hits: 0 });
        }
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
    /// Eine Aenderung am Baum vermerken. `dirty` fuer „zurueckschreiben",
    /// `version` fuer jeden, der einen Zwischenspeicher darauf haelt.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    /// Der LEBENDE Baum, so wie die Kaskade ihn braucht — ohne etwas zu
    /// aendern.
    ///
    /// Unterschied zu `to_dom`: das hier vergibt keine neuen `seq`. `to_dom`
    /// tut das, weil es die Bruecke zum Layout neu spannt; wer es zwischendurch
    /// riefe, um nur EINE Frage zu beantworten, wuerde jedem Kasten im
    /// fertigen Layout die Nummer unter dem Stuhl wegziehen.
    ///
    /// Die Kennung ist stattdessen der ARENA-INDEX, der sich nie aendert. Sie
    /// steht als `seq` im gebauten Element, also findet `find_path` das
    /// Element mit derselben Zahl wieder, mit der der Rufer sein JS-Objekt
    /// haelt.
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
        for (k, v) in &n.attrs { e.attrs.push((k.to_string(), v.to_string())); }
        // MUSS nach den Attributen laufen — siehe `to_node`.
        e.index_attrs();
        for c in &n.children {
            if let Some(x) = self.live_node(*c) { e.children.push(x); }
        }
        Some(crate::dom::Node::Element(e))
    }

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

    /// Der Arena-Knoten zu einer `seq` aus dem Layout.
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
        // Die Bruecke: dieselbe Zahl steht jetzt hier und im Layout.
        self.nodes[id as usize].seq = *seq;
        let mut e = crate::dom::Element::bare(tag.to_string(), *seq);
        for (k, v) in &attrs { e.attrs.push((k.to_string(), v.to_string())); }
        // MUSS nach den Attributen laufen — sonst sind Klassen, id und der
        // Bloom-Filter leer und kein Selektor trifft mehr.
        e.index_attrs();
        for c in kids {
            if let Some(x) = self.to_node(c, seq) { e.children.push(x); }
        }
        Some(crate::dom::Node::Element(e))
    }
}

/// Ein Skript der Seite: entweder sein Text oder die Adresse, unter der er
/// steht.
pub enum ScriptRef {
    /// Quelltext und ob `type="module"` daransteht. Die Fahne gehoert
    /// HIERHER und wird nicht spaeter geraten: ein Modul ohne `import` parst
    /// auch als Skript, haette dann aber den falschen Bereich und das falsche
    /// `this` — und zwar still.
    Inline(String, bool),
    External(String, bool),
}

/// ALLE Skripte der Seite, in Dokumentreihenfolge — eingebettete wie externe.
///
/// Die Reihenfolge ist die des Quelltextes, und beak fuehrt sie auch so aus.
/// Das ist die Bedeutung von `defer` und NICHT die eines blockierenden
/// `<script>` mitten im Koerper: ein Browser wuerde ein klassisches Skript
/// ausfuehren, BEVOR er weiterparst, und `document.write` haengt davon ab.
/// beak hat das Dokument schon fertig, wenn es hier ankommt — also verhaelt
/// sich alles wie `defer`. Bewusst, und es deckt alles ausser `document.write`.
/// Ist `t` das Wurzelelement (`<html>`)?
fn is_root_element(i: &mut Interp, t: &Value) -> bool {
    let Ok(id) = node_of(i, t) else { return false };
    i.doc.as_ref().and_then(|d| d.html) == Some(id)
}

/// Rollt DIESES Element die Seite? Das Wurzelelement und der `<body>` tun
/// es; jedes andere hat in beak keinen eigenen Rollkasten.
///
/// `document.scrollingElement` nennt dasselbe Element, und beide Antworten
/// muessen dieselbe sein: eine Seite liest `scrollingElement` und schreibt
/// dann dessen `scrollTop`.
fn is_scrolling_root(i: &mut Interp, t: &Value) -> bool {
    let Ok(id) = node_of(i, t) else { return false };
    let Some(d) = &i.doc else { return false };
    d.html == Some(id) || d.body == Some(id)
}

/// Die ROLLFLAECHE eines gewoehnlichen Elements: sein Polsterkasten,
/// vereinigt mit den Rahmenkaesten aller Nachfahren.
///
/// Genau das fragt eine Seite, wenn sie `scrollHeight` liest — „passt der
/// Inhalt in den Kasten?". Ohne die Nachfahren waere die Antwort immer ja.
fn scroll_area(i: &Interp, t: &Value) -> Option<(f64, f64)> {
    let id = node_of_ref(i, t).ok()?;
    let g = i.geometry.as_ref()?;
    let d = i.doc.as_ref()?;
    let (own, borders, _) = node_box(i, id)?;
    // Der Polsterkasten ist der Boden: `scrollHeight` ist nie kleiner als
    // `clientHeight`. Gemessen wird von der POLSTERkante aus, also nur den
    // Rahmen hinein — und wir fuehren je Achse nur die SUMME, also ist die
    // halbe die Schaetzung fuer die eine Seite. Bei gleichen Rahmen ist sie
    // exakt; ein Rahmen, der links und rechts verschieden ist, verschiebt sie
    // um wenige Pixel. Gegen die 0, die vorher dastand, ist das keine Frage.
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

/// Die Argumente von `scrollTo`/`scrollBy`: entweder zwei Zahlen oder ein
/// Gegenstand mit `left`/`top`. Was fehlt, bleibt `None` und laesst die
/// Achse in Ruhe — `scrollTo({ top: 0 })` darf nicht waagerecht springen.
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

/// Ein Fenstermass, so wie `set_viewport` es abgelegt hat.
fn viewport_num(i: &mut Interp, key: &str) -> Value {
    let g = Value::Obj(i.realm.global.clone());
    match i.get(&g, key) { Ok(v @ Value::Num(_)) => v, _ => Value::Num(0.0) }
}

/// Dasselbe als Zahl.
fn viewport_f(i: &mut Interp, key: &str) -> f64 {
    match viewport_num(i, key) { Value::Num(n) => n, _ => 0.0 }
}

pub fn page_scripts(d: &Doc) -> Vec<ScriptRef> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    let mut out = Vec::new();
    for id in all {
        let n = &d.nodes[id as usize];
        if &*n.tag != "script" { continue; }
        if !script_type_is_js(n) { continue; }
        let module = n.attr("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("module"));
        match n.attr("src") {
            Some(src) if !src.trim().is_empty() =>
                out.push(ScriptRef::External(src.to_string(), module)),
            _ => {
                let text = d.text_of(id);
                if !text.trim().is_empty() { out.push(ScriptRef::Inline(text, module)); }
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

/// Wie `node_of`, aber `window` zaehlt als das DOKUMENT.
///
/// `window.addEventListener("click", …)` ist die haeufigste Anmeldung
/// ueberhaupt, und im Browser bekommt das Fenster blasende Ereignisse als
/// LETZTES. Der Wurzelknoten steht in jeder Zustellkette genau dort — also
/// ist er die richtige Adresse, nicht eine Naeherung.
fn target_node(i: &mut Interp, v: &Value) -> C<u32> {
    if let Value::Obj(o) = v {
        if Rc::ptr_eq(o, &i.realm.global) {
            return match &i.doc { Some(d) => Ok(d.doc), None => i.type_err("no document") };
        }
    }
    node_of(i, v)
}

/// Welches Stueck eines Dokuments gesucht ist.
#[derive(Clone, Copy, PartialEq)]
enum DocPart { Root, Head, Body }

/// `documentElement` / `head` / `body` — von DIESEM Dokumentknoten aus.
///
/// Fuer das Hauptdokument steht die Antwort gemerkt in `Doc`; fuer jedes
/// andere (`implementation.createHTMLDocument`) wird gelaufen. Zwei Wege und
/// nicht einer, weil der gemerkte auch dann noch stimmt, wenn eine Seite
/// ihren Rumpf umbaut — und weil ein Umbau des Hauptwegs hier nichts
/// gewinnen und alles riskieren wuerde.
fn doc_part(i: &mut Interp, this: &Value, part: DocPart) -> C<Option<u32>> {
    let id = node_of(i, this)?;
    let Some(d) = &i.doc else { return Ok(None) };
    if id == d.doc {
        return Ok(match part { DocPart::Root => d.html, DocPart::Head => d.head, DocPart::Body => d.body });
    }
    // Die Wurzel ist das erste Element unter dem Dokumentknoten.
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
// **Beide haengen an derselben Sache: der Geometrie nach dem Layout.** Der
// Wirt reicht sie mit `Interp::set_geometry` ein, und genau dort werden die
// Beobachter ausgewertet — nicht in einem Zeitgeber, der raet, wann sich
// etwas bewegt haben koennte.
//
// Im Aufrufzensus stehen sie mit 429 (`IntersectionObserver`) und 353
// (`ResizeObserver`) Aufrufen als P4 und P5. Was sie tragen, ist der halbe
// moderne Web-Werkzeugkasten: verzoegert geladene Bilder, unendliche Listen,
// klebende Kopfzeilen, Diagramme, die sich an ihren Kasten anpassen.
//
// **Gemessen wird beim Beobachten, gemeldet spaeter.** Ein Eintrag haelt die
// Kaesten, wie sie ZUM ZEITPUNKT der Beobachtung standen; ihn beim Zustellen
// neu zu rechnen hiesse, dem Rueckruf Zahlen aus einer anderen Runde zu
// geben — und zwischen Beobachtung und Zustellung liegt ein Rueckruf eines
// anderen Beobachters, der den Baum aendern darf.

/// Welchen Kasten ein `ResizeObserver` meldet.
#[derive(Clone, Copy, PartialEq)]
pub enum BoxKind { Content, Border }

/// Eine Anmeldung eines `ResizeObserver`.
pub struct ResizeReg {
    /// Der KNOTEN, nicht sein Layout-`seq`: das Layout vergibt `seq` bei
    /// jedem Lauf neu, und eine Anmeldung ueberlebt jedes Layout.
    pub target: u32,
    pub kind: BoxKind,
    /// Zuletzt GEMELDETE Groesse. `None` heisst „noch nie" — und der erste
    /// Lauf meldet immer, so wie die Spezifikation es verlangt: wer
    /// `observe` ruft, bekommt die aktuelle Groesse, nicht erst die naechste
    /// Aenderung.
    pub last: Option<(f64, f64)>,
}

/// Was ein `ResizeObserverEntry` sagt — beim Beobachten gerechnet.
pub struct ResizeEntry {
    pub target: u32,
    /// Inhaltskasten: Breite, Hoehe.
    pub content: (f64, f64),
    /// Rahmenkasten: Breite, Hoehe.
    pub border: (f64, f64),
}

pub struct ResizeObs {
    pub js: Gc,
    pub cb: Value,
    pub regs: Vec<ResizeReg>,
    pub queue: Vec<ResizeEntry>,
}

/// Eine Anmeldung eines `IntersectionObserver`.
pub struct InterReg {
    pub target: u32,
    /// Der Schwellenindex von letztem Mal: wieviele Schwellen das Verhaeltnis
    /// erreicht hat. Gemeldet wird, wenn sich DIESE Zahl aendert — nicht
    /// jedes Pixel, sonst waere jeder Bildlauf ein Rueckrufgewitter.
    /// `-1` heisst „noch nie gemeldet".
    pub band: i32,
}

/// Was ein `IntersectionObserverEntry` sagt.
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
    /// `None` heisst: das Sichtfeld. Ein anderes Element als Wurzel ist
    /// erlaubt und wird hier genauso behandelt — sein Kasten ist dann der
    /// Ausschnitt.
    pub root: Option<u32>,
    /// Der ungelesene `rootMargin`-Text, damit der Getter ihn zurueckgeben
    /// kann, ohne aus vier Zahlen wieder Text zu raten.
    pub margin_src: Rc<str>,
    /// `rootMargin` in Pixeln, oben/rechts/unten/links. Prozente werden beim
    /// Anmelden aufgeloest; die Spezifikation erlaubt beides.
    pub margin: (f64, f64, f64, f64),
    pub thresholds: Vec<f64>,
    pub regs: Vec<InterReg>,
    pub queue: Vec<InterEntry>,
}

/// Der Layout-`seq` eines Knotens — dieselbe Auskunft wie `layout_seq`, nur
/// von der id aus. Die Beobachter kennen ihre Ziele als Knoten.
fn node_seq(i: &Interp, id: u32) -> Option<u32> {
    let n = i.doc.as_ref()?.nodes.get(id as usize)?;
    Some(if n.seq != 0 { n.seq } else { n.src_seq }).filter(|s| *s != 0)
}

/// Der Rahmenkasten eines Knotens in FENSTERkoordinaten — die Vereinigung
/// seiner Fragmente, genau wie `getBoundingClientRect`, dazu die Rahmen- und
/// Polstersummen des ersten Fragments, GETRENNT.
///
/// Getrennt, weil zwei Fragen daran haengen und sie verschiedene Kanten
/// meinen: `contentRect` will beide abziehen, `scrollHeight` misst ab der
/// POLSTERkante und zieht nur den Rahmen ab. Zusammengefasst waren sie
/// einmal, und der Unterschied war 5 px, die kein Test bemerkt haette, wenn
/// er nicht beides in demselben Kasten gehabt haette.
///
/// Dieselbe Rechnung wie `elem_rect`, nur ohne den Umweg ueber einen
/// JS-Wert. Zwei Rechenwege waeren zwei Wahrheiten ueber denselben Kasten
/// ([[feedback_intrinsic_shared_path]]).
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

/// `rootMargin`: ein bis vier CSS-Laengen, oben/rechts/unten/links wie bei
/// `margin`. Prozente stehen zur AUSSCHNITTgroesse — waagerecht zur Breite,
/// senkrecht zur Hoehe, so wie bei jedem anderen Rand auch.
fn parse_root_margin(s: &str, vw: f64, vh: f64) -> (f64, f64, f64, f64) {
    let one = |t: &str, basis: f64| -> f64 {
        let t = t.trim();
        if let Some(n) = t.strip_suffix('%') {
            n.trim().parse::<f64>().map(|v| v / 100.0 * basis).unwrap_or(0.0)
        } else if let Some(n) = t.strip_suffix("px") {
            n.trim().parse::<f64>().unwrap_or(0.0)
        } else {
            // Eine blanke Zahl ist KEINE Laenge (nur `0` waere eine), und eine
            // Einheit, die wir nicht kennen, auch nicht. Beides wird 0 statt
            // geraten.
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

/// Beide Beobachter auswerten. Gerufen aus `Interp::set_geometry` — dem
/// einen Moment, in dem der Wirt sagt „so steht die Seite jetzt".
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
            // Ein Beobachter meldet AENDERUNGEN — und beim ersten Mal die
            // Lage, wie sie ist.
            if last != Some(seen) {
                i.resize_obs[n].regs[r].last = Some(seen);
                i.resize_obs[n].queue.retain(|e| e.target != id);
                i.resize_obs[n].queue.push(ResizeEntry { target: id, content, border });
            }
        }
    }

    // ── IntersectionObserver ─────────────────────────────────────────────
    let (vw, vh) = i.viewport;
    let now = { i.fake_now += 1.0; i.fake_now };
    for n in 0..i.inter_obs.len() {
        // Der Ausschnitt: das Sichtfeld oder der Kasten der Wurzel, in
        // beiden Faellen um `rootMargin` gedehnt.
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
            // Ein Kasten ohne Flaeche (eine leere Zeile, ein umbrochener
            // Inline-Kasten ohne Breite) hat kein Verhaeltnis — er schneidet,
            // wenn er im Ausschnitt LIEGT. Sonst waere 0/0 die Antwort.
            let inside = tx >= rx0 && tx <= rx1 && ty >= ry0 && ty <= ry1;
            let (hit, ratio) = if area > 0.0 {
                (overlap, if overlap { iw * ih / area } else { 0.0 })
            } else {
                (inside, if inside { 1.0 } else { 0.0 })
            };
            // Wieviele Schwellen erreicht sind. Die Schwelle 0 gilt erst als
            // erreicht, wenn ueberhaupt geschnitten wird — sonst waere jedes
            // Element von Anfang an „ueber 0".
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

/// Aus einem `ResizeEntry` das JS-Objekt bauen.
fn build_resize_entry(i: &mut Interp, e: &ResizeEntry) -> Value {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    let target = wrap(i, e.target);
    // `contentRect` steht im Polsterkasten: x/y sind die Polsterung links
    // und oben. Wir fuehren nur die SUMMEN, also die halbe — richtig fuer
    // gleichmaessige Polsterung und nie schlechter als die 0, die vorher
    // dagestanden haette.
    let rect = rect_obj(i, Some((0.0, 0.0, e.content.0, e.content.1)));
    let size = |i: &mut Interp, (w, h): (f64, f64)| -> Value {
        let s = new_obj(Some(i.realm.object_proto.clone()));
        s.borrow_mut().define("inlineSize", Prop::data(Value::Num(w)));
        s.borrow_mut().define("blockSize", Prop::data(Value::Num(h)));
        // Eine Liste, weil ein Element in einem fragmentierten Kasten
        // mehrere Groessen haette. Wir haben immer genau eine — die Liste
        // ist trotzdem die richtige Form, denn Seitencode schreibt
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
        // Wir kennen kein Geraetepixel-Raster, also ist die Liste leer statt
        // erfunden.
        b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("ResizeObserverEntry")));
    }
    let empty = i.new_array(Vec::new());
    o.borrow_mut().define("devicePixelContentBoxSize", Prop::data(empty));
    Value::Obj(o)
}

/// Aus einem `InterEntry` das JS-Objekt bauen.
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

/// Der Kontrollpunkt: wer etwas in der Schlange hat, wird gerufen. Neben
/// `deliver_mutations` und aus demselben Grund dort — nach jedem
/// Einstiegspunkt, nicht in einem Zeitgeber.
///
/// Liefert true, wenn ein Rueckruf gelaufen ist.
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

/// Eine Anmeldung eines `MutationObserver`: WAS er an WELCHEM Knoten sehen
/// will (DOM §4.3.1 „registered observer").
pub struct MutReg {
    pub target: u32,
    /// Der Platz dieser Anmeldung in `Doc::observed` — das Bit, das eine
    /// Aenderung gesetzt hat, wenn sie diesen Knoten betraf.
    pub slot: usize,
    pub subtree: bool,
    pub child_list: bool,
    pub attrs: bool,
    pub attr_old: bool,
    pub char_data: bool,
    pub char_old: bool,
    /// `attributeFilter` — nur diese Attribute. `None` heisst „alle".
    pub filter: Option<Vec<Rc<str>>>,
}

/// Ein angemeldeter `MutationObserver`.
///
/// Er liegt im `Interp` und nicht im `Doc`, weil er einen JS-Rueckruf haelt:
/// das Dokument wird bei jeder Navigation neu gebaut, der Realm nicht.
pub struct MutObs {
    /// Das JS-Objekt — die IDENTITAET. Ueber sie findet `observe` den
    /// richtigen Eintrag wieder.
    pub js: Gc,
    pub cb: Value,
    pub regs: Vec<MutReg>,
    pub queue: Vec<Mutation>,
}

/// Sieht noch jemand zu? Wenn nicht, hoert der Baum auf aufzuzeichnen.
fn sync_observing(i: &mut Interp) {
    // Die Anmeldungen bekommen ihre Plaetze — und der Baum die Liste, die er
    // beim Aufzeichnen braucht.
    let mut obs: Vec<(u32, bool)> = Vec::new();
    let mut voll = false;
    for o in i.observers.iter_mut() {
        for r in o.regs.iter_mut() {
            match obs.iter().position(|(t, s)| *t == r.target && *s == r.subtree) {
                Some(k) => r.slot = k,
                None if obs.len() < 64 => { r.slot = obs.len(); obs.push((r.target, r.subtree)); }
                // **Mehr als 64 verschiedene beobachtete Knoten.** Gesagt
                // statt verschluckt: die Maske ist ein `u64`, und eine
                // Anmeldung ohne Platz meldet nichts.
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

/// Passt diese Aenderung zu dieser Anmeldung?
///
/// Die ZUGEHOERIGKEIT steht schon fest — sie wurde beim Aendern gerechnet
/// (`Mutation::hits`). Hier faellt nur noch, was diese Anmeldung inhaltlich
/// nicht sehen will.
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

/// Die Rohaufzeichnung des Baums auf die Beobachter verteilen.
///
/// **Erst hier wird gefiltert, nicht beim Aufzeichnen.** Der Baum weiss
/// nicht, wer zusieht, und soll es nicht wissen muessen; und dieselbe
/// Aenderung kann an mehrere Beobachter gehen.
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
            // Der Beobachter bekommt die Aenderung EINMAL, auch wenn zwei
            // seiner Anmeldungen passen — sonst saehe eine Seite, die
            // Elter und Kind beobachtet, alles doppelt.
            if o.regs.iter().any(|r| matches_reg(m, r)) {
                // `oldValue` gibt es nur, wenn die Anmeldung danach
                // gefragt hat. Ihn immer mitzuliefern waere bequem und
                // falsch: Seiten unterscheiden `null` von `""`.
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

/// Aus einer Aenderung ein `MutationRecord` bauen.
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
        // Ein Namensraum, den wir nicht fuehren, ist `null` — und `null` ist
        // hier die richtige Antwort, nicht eine fehlende.
        b.define("attributeNamespace", Prop::data(Value::Null));
        b.define("oldValue", Prop::data(match &m.old {
            Some(v) => Value::str(v), None => Value::Null }));
        b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("MutationRecord")));
    }
    Ok(Value::Obj(o))
}

/// Der Kontrollpunkt: einsammeln, und wer etwas hat, wird gerufen.
///
/// Liefert true, wenn ein Rueckruf gelaufen ist — der Rufer muss dann noch
/// einmal vorbeikommen, weil ein Beobachter im Rueckruf den Baum aendern darf.
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
        // Wirft der Rueckruf, ist das SEIN Fehler und nicht das Ende der
        // Zustellung: die anderen Beobachter bekommen ihre Meldungen
        // trotzdem, so wie im Browser.
        if let Err(e) = i.call(&cb, this.clone(), &[arr, this]) {
            let msg = super::modules::describe(i, e);
            i.console_push(alloc::format!("error: MutationObserver-Rueckruf: {msg}"));
        }
    }
    ran
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
        COMMENT_NODE => i.realm.comment_proto.clone(),
        _ => {
            let tag = i.doc.as_ref().map(|d| d.nodes[id as usize].tag.clone())
                       .unwrap_or_else(|| Rc::from(""));
            if &*tag == "#fragment" { i.realm.fragment_proto.clone() }
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

/// Die Schlitze eines Ereignisses. Nicht aufzaehlbar und mit `__` davor:
/// ein `for (k in e)` einer Seite darf sie nicht sehen, und `e.type` kommt
/// vom Prototyp, nicht von der Instanz.
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

/// Ein Getter, das einen festen Schlitz liest. Als Makro, weil ein
/// eingebautes Getter ein FUNKTIONSZEIGER ist: er faengt nichts ein, also
/// muss der Schlitzname im Rumpf stehen und nicht in einer Variablen.
macro_rules! ev_getter {
    ($proto:expr, $fp:expr, $name:literal, $slot:expr) => {{
        let g = native(Some($fp.clone()), |i, t, _| i.get(&t, $slot),
                       concat!("get ", $name), 0, false);
        $proto.borrow_mut().define($name, Prop { value: None, get: Some(Value::Obj(g)),
            set: None, writable: false, enumerable: false, configurable: true });
    }};
}

/// Ein Ereignisobjekt mit gesetzten Schlitzen. `trusted` unterscheidet, was
/// beak selbst zustellt, von dem, was die Seite mit `dispatchEvent` schickt —
/// Seiten fragen es ab, und ein festes `true` waere gelogen.
fn build_event(i: &mut Interp, proto: Gc, kind: &str, trusted: bool) -> Gc {
    let ev = new_obj(Some(proto));
    let stamp = { i.fake_now += 1.0; i.fake_now };
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

/// `new Event(art, {bubbles, cancelable})` — das zweite Argument.
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

/// Sind das dieselbe Funktion? Identitaet, nicht Gleichheit — genau das
/// fragt `removeEventListener`.
fn same_fn(a: &Value, b: &Value) -> bool {
    match (a, b) { (Value::Obj(x), Value::Obj(y)) => Rc::ptr_eq(x, y), _ => false }
}

/// Die Zustellkette fuer einen Knoten: von der Wurzel bis zu ihm.
///
/// Dieselbe Reihenfolge, in der beak sie aus dem LAYOUT baut — aussen zuerst,
/// Ziel zuletzt. Wer das dreht, dreht die Blasenrichtung.
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

/// Die Erklaerungen eines `style`-Attributs, in der Reihenfolge des Textes.
///
/// Ein eigener kleiner Leser und nicht der aus `css`: der hier bekommt genau
/// das zurueckzugeben, was ein Skript hineingeschrieben hat — der Kaskadenleser
/// wirft ungueltige Erklaerungen weg, und dann laese `el.style.foo` etwas
/// anderes als das eben Geschriebene.
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

/// Der Schnappschuss, den `getComputedStyle` hinterlegt. Ist er da, liest die
/// Sicht IHN statt des `style`-Attributs — dieselben Zugriffsfunktionen, zwei
/// Quellen, und keine zweite Maschinerie.
const COMPUTED: &str = "__computed";

/// Der Deklarationstext hinter einer `CSSStyleDeclaration`.
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

/// Wie `node_of`, aber ohne zu werfen — ein Schnappschuss hat keinen Knoten.
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
        // Nicht gesetzt ist die LEERE Zeichenkette, nicht `undefined` — so
        // steht es in der Spezifikation, und Seiten pruefen darauf.
        None => Value::str(""),
    }
}

/// Eine Erklaerung setzen oder (bei leerem Wert) entfernen.
///
/// Das Ergebnis landet im `style`-ATTRIBUT, nicht in einer Nebenablage: die
/// Kaskade liest das Attribut, also wirkt `el.style.display = "none"` damit
/// wirklich — vorher lief die Zuweisung ins Leere und die Seite blieb stehen,
/// wie sie war.
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

/// Ein Eigenschaftspaar auf `CSSStyleDeclaration.prototype`. Als Makro aus
/// demselben Grund wie `ev_getter`: ein eingebautes Getter ist ein
/// Funktionszeiger und faengt nichts ein.
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

/// Ein Behandler als Eigenschaft: `el.onclick`. Als Makro, weil das Getter
/// ein Funktionszeiger ist und die Ereignisart im Rumpf stehen muss.
macro_rules! handler_prop {
    ($proto:expr, $fp:expr, $js:literal, $kind:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            let id = target_node(i, &t)?;
            if let Some(f) = i.doc.as_ref().and_then(|d| d.nodes[id as usize].handlers.iter()
                .find(|(k, _)| &**k == $kind).map(|(_, f)| f.clone())) { return Ok(f) }
            // Steht nur das Attribut da, gibt der Browser trotzdem eine
            // FUNKTION zurueck — also uebersetzen wir es hier, wie beim
            // Ausloesen auch.
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
                    // Ohne diese Zeile zeichnet das Layout keine
                    // Treffer-Kaesten auf, und der Klick findet nichts —
                    // dieselbe Falle wie bei `addEventListener`.
                    d.has_listeners = true;
                }
            }
            Ok(Value::Undefined)
        }, concat!("set ", $js), 1, false);
        $proto.borrow_mut().define($js, Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(s)), writable: false, enumerable: false, configurable: true });
    }};
}

/// Ein Feld, das auf einem ATTRIBUT sitzt: `a.href`, `img.src`, `el.title`.
/// Lesen gibt die leere Zeichenkette, wenn es das Attribut nicht gibt —
/// nicht `undefined`, denn darauf ruft Seitencode `.indexOf`.
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

/// Wie `attr_prop!`, aber `null` statt der leeren Zeichenkette, wenn das
/// Attribut fehlt. So spiegeln die ARIA-Felder (ARIA 1.2 §9): sie sind
/// `DOMString?`, und `el.ariaHidden === null` ist die Frage „steht da
/// ueberhaupt etwas".
macro_rules! attr_prop_null {
    ($proto:expr, $fp:expr, $js:literal, $attr:literal) => {{
        let g = native(Some($fp.clone()), |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr($attr) {
                Some(v) => Value::Str(v.clone()), None => Value::Null }))
        }, concat!("get ", $js), 0, false);
        let s = native(Some($fp.clone()), |i, t, a| {
            let id = node_of(i, &t)?;
            match a.first() {
                // `el.ariaHidden = null` NIMMT das Attribut weg. Es auf die
                // Zeichenkette "null" zu setzen waere die naheliegende
                // Bequemlichkeit und ein sichtbarer Fehler: `aria-hidden="null"`
                // ist wahr, weil jeder Wert ausser "false" wahr ist.
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

/// Ein Feld, das eine ZAHL auf einem Attribut ist: `el.tabIndex`. Fehlt das
/// Attribut oder ist es keine Zahl, gilt `default` — bei `tabIndex` ist das
/// nicht 0, sondern -1 fuer alles, was nicht von sich aus anspringbar ist.
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

/// Ein Feld, das die ANWESENHEIT eines Attributs ist: `el.hidden`,
/// `script.async`. Der Wert des Attributs zaehlt nicht — `hidden="false"`
/// versteckt trotzdem, so steht es im HTML.
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

/// Die Knoten eines geparsten BRUCHSTUECKS: der `<html>`/`<head>`/`<body>`-
/// Rahmen, den der Dokumentparser immer baut, faellt weg.
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

/// Die Nummer, unter der das LAYOUT dieses Element kennt.
///
/// Zwei Zahlen kommen in Frage, und welche gilt, haengt am Zeitpunkt:
/// `to_dom` vergibt beim Zurueckschreiben frische `seq`, davor sind sie 0 und
/// das Layout stammt noch aus dem geparsten Baum — dessen Nummern stehen in
/// `src_seq`. „Nicht-null gewinnt" ist damit keine Heuristik, sondern die
/// Frage „hat schon einmal jemand zurueckgeschrieben?".
fn layout_seq(i: &Interp, this: &Value) -> Option<u32> {
    let id = node_of_ref(i, this).ok()?;
    let n = &i.doc.as_ref()?.nodes[id as usize];
    Some(if n.seq != 0 { n.seq } else { n.src_seq }).filter(|s| *s != 0)
}

/// Der Rahmenkasten in FENSTERkoordinaten: `(x, y, w, h)`.
///
/// Ein Kasten kann in mehrere Fragmente zerfallen (ein Inline-Kasten je
/// Zeile) — `getBoundingClientRect` nennt deren Vereinigung, und das ist
/// genau das, was ein Browser dort auch liefert.
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

/// Der POLSTERkasten: Breite und Hoehe ohne die Rahmen.
fn elem_inner(i: &Interp, this: &Value) -> Option<(f64, f64)> {
    let g = i.geometry.as_ref()?;
    let seq = layout_seq(i, this)?;
    let (_, _, w, h) = elem_rect(i, this)?;
    // Die Rahmen des ERSTEN Fragments: ein ueber Zeilen gebrochener Kasten
    // zeichnet sie nur an seinen aeusseren Enden, und dort steht ohnehin 0.
    let b = g.boxes.iter().find(|b| b.seq == seq)?;
    Some(((w - b.bx as f64).max(0.0), (h - b.by as f64).max(0.0)))
}

/// Ein `DOMRect`-artiger Gegenstand. `None` heisst „kein Kasten" und wird zu
/// lauter Nullen — dieselbe Antwort, die ein Browser fuer ein Element ohne
/// Kasten (`display:none`) gibt.
fn rect_obj(i: &Interp, r: Option<(f64, f64, f64, f64)>) -> Gc {
    let (x, y, w, h) = r.unwrap_or((0.0, 0.0, 0.0, 0.0));
    let o = new_obj(Some(i.realm.object_proto.clone()));
    for (k, v) in [("x", x), ("y", y), ("left", x), ("top", y),
                   ("right", x + w), ("bottom", y + h), ("width", w), ("height", h)] {
        o.borrow_mut().define(k, Prop::data(Value::Num(v)));
    }
    o
}

/// Wohin `append` & Co. einhaengen.
enum Where { First, Last, Before, After }

/// Der gemeinsame Rumpf von `append`/`prepend`/`before`/`after`. Ein
/// Argument, das kein Knoten ist, wird zum Textknoten — genau das
/// unterscheidet diese Familie von `appendChild`.
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
        // Der Anker bleibt derselbe: alles landet DAVOR, also stehen mehrere
        // Argumente am Ende in der Reihenfolge, in der sie uebergeben wurden.
        if let Some(d) = &mut i.doc { d.insert_maybe_fragment(parent, id, anchor); d.touch(); }
        fire_connected(i, id)?;
    }
    Ok(Value::Undefined)
}

/// Den KASKADIERTEN Stil eines Elements als Deklarationstext.
///
/// Die Kaskade laeuft auf beaks Baum, nicht auf der Arena der Maschine — also
/// wird das Element ueber `src_seq` dort gesucht und die Kette von der Wurzel
/// herunter aufgeloest. Das kostet einen Lauf je Ebene (auf einer echten
/// Seite ein Dutzend), und zwar je Aufruf: `getComputedStyle` ist eine Frage
/// an den JETZIGEN Zustand, und ein Zwischenspeicher muesste wissen, wann er
/// falsch wird.
///
/// `None`, wenn kein Kontext eingereicht wurde oder das Element im Baum nicht
/// vorkommt (ein Skript hat es erst erzeugt) — dann bleibt es beim
/// Inline-Stil.
/// Der Baum, auf dem die Kaskade rechnet: der LEBENDE, aus `doc` gebaut und
/// nur dann neu gebaut, wenn `doc.version` sich bewegt hat.
///
/// Ein Skript, das eine Klasse setzt und dann misst, ist kein Randfall — und
/// aus einem Schnappschuss beantwortet, waeren es zwei Antworten auf dieselbe
/// Frage. Der Zwischenspeicher ist der Preis dafuer: EIN Aufbau je
/// Aenderungsschub, nicht je Abfrage.
fn style_tree(i: &Interp) -> Option<alloc::rc::Rc<crate::dom::Dom>> {
    let doc = i.doc.as_ref()?;
    let mut slot = i.live_dom.borrow_mut();
    if !matches!(&*slot, Some((v, _)) if *v == doc.version) {
        *slot = Some((doc.version, alloc::rc::Rc::new(doc.live_dom())));
    }
    slot.as_ref().map(|(_, d)| d.clone())
}

/// `node` ist der ARENA-INDEX des Elements — dieselbe Zahl, die `live_dom`
/// als `seq` in den Baum schreibt.
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
    // Die Variablenkarte faehrt MIT. Ohne sie erreicht `:root`s Palette das
    // Element nie — und ein Rahmenwerk, das seine ganze Skala ueber
    // Variablen fuehrt (Tailwind: `font-size: var(--text-xs)`), bekaeme
    // ueberall die Vorgabewerte zurueck. `getComputedStyle` haette dann eine
    // andere Antwort gegeben als das Layout gemalt hat, und das ist die
    // schlimmste Sorte Fehler: zwei Wahrheiten.
    let mut vars = crate::vars::VarMap::new();
    let mut out = parent;
    for (k, el) in path.iter().enumerate() {
        // Geschwister zaehlen, damit `:nth-*` und `:first-child` stimmen —
        // sonst haette der gerechnete Stil eine andere Kaskade gesehen als
        // das Layout.
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
        if let Some(m) = own { vars = m; }
        // `rem` rechnet gegen die WURZEL, und die steht erst fest, wenn sie
        // aufgeloest ist. Das Layout setzt das direkt nach dem Wurzellauf;
        // ohne die Zeile las `getComputedStyle` jedes `rem` gegen die
        // Vorgabegroesse — `font-size: .75rem` kam als 16 px zurueck, waehrend
        // das Layout 12 malte. Zwei Antworten auf dieselbe Frage.
        if k == 0 {
            out.rem_base = out.font_px;
        }
        parent = out;
        anc.push(info);
    }
    Some(crate::style::serialize_computed(&out))
}

/// Den Weg von der Wurzel zu `seq` sammeln.
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

/// Eine Funktion auf dem Fenster. `meth` legt sie auf einen Prototyp, hier
/// gehoert sie an den globalen Gegenstand selbst.
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
// **Warum es das gibt, und warum in dieser Groesse.** htmx sucht seine
// `hx-on:`-Attribute mit einem XPath-Ausdruck, und es war das letzte der
// dreizehn Bibliotheksproben, das rot stand. Der Chromium-Zensus ueber zwoelf
// echte Zielseiten zaehlt dagegen NULL XPath-Aufrufe: das hier ist keine
// Web-Anforderung nach Aufrufzahl, sondern eine BIBLIOTHEKS-Anforderung.
//
// Diese Messung entscheidet die Form, nicht das Ob. Ein Sonderfall fuer htmx'
// einen Ausdruck waere ein Notnagel an der Stelle eines fehlenden Merkmals
// ([[feedback_a_workaround_is_the_wrong_answer_to_a_missing_capability]]);
// volles XPath 1.0 mit Namensraeumen waere Gold, nach dem niemand fragt.
// Gebaut ist die Sprache, die eine Seite wirklich schreibt — `js/xpath.rs`
// sagt, was fehlt.

/// Den Ausdruck holen — einmal geparst, unter seinem Quelltext gemerkt.
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
        // Ein unlesbarer Ausdruck ist ein FEHLER, keine leere Treffermenge:
        // eine leere Menge sieht aus wie „nichts gefunden".
        Err(e) => Err(i.throw_kind("SyntaxError", &alloc::format!("XPath: {e}"))),
    }
}

/// Ein Attributknoten als `Attr`-Objekt — dieselbe Form, die
/// `element.attributes` liefert.
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

/// `document.evaluate` / `XPathExpression.evaluate` — beide enden hier.
fn xpath_run(i: &mut Interp, src: &str, ctx: &Value, want: f64) -> C<Value> {
    let id = node_of(i, ctx)?;
    let expr = xpath_compiled(i, src)?;
    let Some(doc) = i.doc.as_ref() else { return i.type_err("XPath: no document") };
    let val = expr.eval(doc, super::xpath::XNode::Node(id));
    // Erst auswerten, dann huellen: die Auswertung leiht `i.doc` aus, das
    // Huellen braucht `i` veraenderlich.
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
    // `ANY_TYPE` (0) meldet den natuerlichen Typ des Ergebnisses; sonst
    // gilt, was der Aufrufer verlangt hat.
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

fn install_xpath(realm: &mut Realm) {
    let fp = realm.function_proto.clone();

    // ── XPathResult ──────────────────────────────────────────────────────
    let res_proto = new_obj(Some(realm.object_proto.clone()));
    let res_ctor = native(Some(fp.clone()),
        |i, _, _| i.type_err("Illegal constructor"), "XPathResult", 0, true);
    res_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(res_proto.clone())));
    res_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(res_ctor.clone())));
    res_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("XPathResult")));
    // Die zehn Typkonstanten stehen im Browser auf BEIDEN — der Konstruktor
    // ist die uebliche Schreibweise (`XPathResult.FIRST_ORDERED_NODE_TYPE`),
    // die Instanz die seltenere.
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
            // Erschoepft: `null`, und darauf endet die `while`-Schleife, mit
            // der jeder Aufrufer darueber laeuft.
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
        // JETZT parsen, nicht erst beim Auswerten: `createExpression` ist die
        // Stelle, an der ein Browser einen Syntaxfehler meldet, und eine Seite
        // die ihren Ausdruck beim Laden baut soll ihn beim Laden hoeren.
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
    // `createNSResolver` gibt es, damit der uebliche Vieraufruf nicht wirft;
    // Namensraeume loest es nicht auf, und das steht in `js/xpath.rs`.
    meth(&ev_proto, "createNSResolver", |_, _, a| {
        Ok(a.first().cloned().unwrap_or(Value::Null))
    }, 1, &fp);
    realm.xpath_eval_proto = ev_proto;
}

/// Baut `Node`/`Element`/`Document`-Prototypen und das globale `document`.
pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    // `EventTarget` steht UNTER `Node`: `addEventListener` gehoert dorthin,
    // nicht auf den Knoten. Sonst hat `window` es nicht — und `window
    // .addEventListener` ist mit 33 360 Aufrufen der haeufigste DOM-Aufruf
    // des ganzen Zielkorpus (`tools/jsscope/out/apicensus.json`).
    let event_target_proto = new_obj(Some(realm.object_proto.clone()));
    let node_proto = new_obj(Some(event_target_proto.clone()));
    let element_proto = new_obj(Some(node_proto.clone()));
    let text_proto = new_obj(Some(node_proto.clone()));
    let document_proto = new_obj(Some(node_proto.clone()));
    // `DocumentFragment` haengt an Node, nicht an Element — hier oben, weil
    // die Suchfunktionen weiter unten auch auf ihm sitzen.
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
    // `normalize` — benachbarte Textknoten verschmelzen, leere entfernen.
    //
    // Die Fritzbox ruft es am Ende JEDES Anhaengens. Ohne die Methode wirft
    // dort „normalize is not a function", und zwar nachdem die Kinder schon
    // dranhaengen: der Baum ist dann halb gebaut und die Meldung zeigt auf
    // die falsche Stelle.
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
    accessor(&node_proto, "nodeValue",
        // Ein Element HAT keinen Wert — `null` ist die Antwort, nicht "".
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
    // Die Bitmaske aus der Spezifikation. Seiten benutzen sie fuer genau eine
    // Frage — „liegt A vor B?" — und `& 4` ist die Art, sie zu stellen.
    meth(&node_proto, "compareDocumentPosition", |i, t, a| {
        let x = node_of(i, &t)?;
        let y = node_of(i, a.first().unwrap_or(&Value::Undefined))?;
        if x == y { return Ok(Value::Num(0.0)) }
        let Some(d) = &i.doc else { return Ok(Value::Num(1.0)) };
        let up = |mut n: u32| { let mut v = alloc::vec![n];
            while let Some(p) = d.nodes[n as usize].parent { v.push(p); n = p; } v.reverse(); v };
        let (ax, ay) = (up(x), up(y));
        if ax[0] != ay[0] { return Ok(Value::Num(1.0 + 2.0 + 32.0)) }   // DISCONNECTED
        // Der erste Punkt, an dem die Wege sich trennen, entscheidet.
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
    // Anmelden, aber noch nicht zustellen. Ein `addEventListener`, das WIRFT,
    // beendet das Skript — eins, das die Anmeldung nur aufbewahrt, laesst es
    // weiterlaufen. Die Zustellung setzt genau hier an.
    meth(&event_target_proto, "addEventListener", |i, t, a| {
        let id = target_node(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if let Some(d) = &mut i.doc {
            d.nodes[id as usize].listeners.push((ev, f));
            // Sobald EIN Behandler da ist, braucht das Layout Treffer-Kaesten.
            d.has_listeners = true;
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    // `removeEventListener(art, f)` nimmt GENAU f weg, nicht alles dieser
    // Art. Vorher fiel mit einem `resize`-Behandler jeder zweite mit ab —
    // und eine Seite, die einen von dreien abmeldet, verlor alle drei.
    meth(&event_target_proto, "removeEventListener", |i, t, a| {
        let id = target_node(i, &t)?;
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if let Some(d) = &mut i.doc {
            d.nodes[id as usize].listeners.retain(|(e, g)| *e != ev || !same_fn(g, &f));
        }
        Ok(Value::Undefined)
    }, 2, &fp);
    // Die Seite loest selbst aus: `el.dispatchEvent(new Event("change"))`.
    // Vorher gab das ein festes `true` zurueck, ohne einen Behandler zu
    // rufen — eine Antwort, die aussieht wie eine Zustellung.
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
        // Blast es nicht, ist die Kette genau ein Knoten lang — dann laeuft
        // nur der Behandler am Ziel, und das ist der ganze Unterschied.
        let chain = if bubbles { ancestors(i, id) } else { alloc::vec![id] };
        let prevented = deliver(i, &ev, &kind, &chain)?;
        Ok(Value::Bool(!prevented))
    }, 1, &fp);

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
    // ── Geometrie ────────────────────────────────────────────────────────
    //
    // Bis 0.75.0 stand hier ueberall eine 0, und der Kommentar begruendete das
    // damit, dass beak erst NACH den Skripten auslegt. Der Grund war einmal
    // richtig und ist es seit der Ereigniszustellung nicht mehr: ein
    // Klickbehandler laeuft auf einer fertig ausgelegten Seite.
    //
    // **Eine 0 war dabei das teuerste, was hier stehen konnte.** Sie wirft
    // nicht, sie steht in keinem Log, sie sieht aus wie eine Antwort — der
    // Tooltip landet in der Ecke, die Sichtbarkeitspruefung haelt alles fuer
    // sichtbar. Im Aufrufzensus ist `getBoundingClientRect` mit 1125 Aufrufen
    // der GROESSTE einzelne Posten, und er stand als „gedeckt" in der Bilanz.
    meth(&element_proto, "getBoundingClientRect", |i, t, _| {
        let r = elem_rect(i, &t);
        Ok(Value::Obj(rect_obj(i, r)))
    }, 0, &fp);
    // Die Fragmente einzeln — ein Inline-Kasten ueber drei Zeilen hat drei
    // Rechtecke, und genau deshalb gibt es diese Funktion neben der oberen.
    meth(&element_proto, "getClientRects", |i, t, _| {
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
        |i, t, _| Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.2))), &fp);
    getter(&element_proto, "offsetHeight",
        |i, t, _| Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.3))), &fp);
    // `offsetTop`/`offsetLeft` gehen gegen den `offsetParent`, und den gibt es
    // hier nicht. Gegen das DOKUMENT ist die naechstbeste Wahrheit und fuer
    // die ueblichen Faelle (ein Element in einem nicht positionierten Rumpf)
    // dieselbe Zahl. Benannt, damit niemand sie fuer exakt haelt.
    getter(&element_proto, "offsetTop", |i, t, _| {
        let sy = i.geometry.as_ref().map_or(0, |g| g.scroll.1);
        Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.1 + sy as f64)))
    }, &fp);
    getter(&element_proto, "offsetLeft", |i, t, _| {
        let sx = i.geometry.as_ref().map_or(0, |g| g.scroll.0);
        Ok(Value::Num(elem_rect(i, &t).map_or(0.0, |r| r.0 + sx as f64)))
    }, &fp);
    // `clientWidth`/`clientHeight` sind der POLSTERkasten: der Rahmenkasten
    // ohne die Rahmen. Die Summen faehrt `HoverBox` mit.
    //
    // **Ausser am WURZELELEMENT — dort sind sie die Sichtflaeche** (CSSOM
    // View §4: „If the element is the root element … return the viewport
    // width/height"). Das ist keine Feinheit: Googles Startseite misst damit
    // das Fenster und laeuft nur weiter, wenn beide Werte wahr sind —
    //
    //     h = p.clientWidth; m = p.clientHeight;
    //     if (h && m && …) { … "/client_204?…&biw=" + h + "&bih=" + m … }
    //
    // Mit 0 blieb der Block stehen, das Formular schickte `biw=&bih=`, und
    // Google hielt uns fuer einen Browser ohne JavaScript.
    getter(&element_proto, "clientWidth", |i, t, _| {
        if is_root_element(i, &t) { return Ok(viewport_num(i, "innerWidth")) }
        Ok(Value::Num(elem_inner(i, &t).map_or(0.0, |(w, _)| w)))
    }, &fp);
    getter(&element_proto, "clientHeight", |i, t, _| {
        if is_root_element(i, &t) { return Ok(viewport_num(i, "innerHeight")) }
        Ok(Value::Num(elem_inner(i, &t).map_or(0.0, |(_, h)| h)))
    }, &fp);
    // ── Die Rollmasse ────────────────────────────────────────────────────
    //
    // **Sie waren da und antworteten 0** — 582 Aufrufe im Zensus, die
    // groesste Position, die keine neue Schnittstelle braucht, sondern eine
    // Leitung. Der Kommentar, der hier stand, war ehrlich: die Zahlen gab es
    // im Layout nicht. Jetzt gibt es sie.
    //
    // **beak klemmt nichts ab.** `overflow: auto`/`scroll` schneidet hier
    // nicht, es gibt keine Rollkaesten je Element (`layout.rs`: „we have no
    // scroll containers"). Das ist keine Ausrede, sondern die Antwort:
    //
    // * `scrollTop`/`scrollLeft` sind an jedem gewoehnlichen Element **0**,
    //   und das ist WAHR, nicht geraten — nichts an ihm ist weggerollt. Am
    //   Wurzelelement und am `<body>` sind sie der Rollstand der SEITE, denn
    //   die rollt.
    // * `scrollHeight`/`scrollWidth` sind die Rollflaeche: der Polsterkasten,
    //   vereinigt mit den Rahmenkaesten aller Nachfahren. Ein Element mit
    //   `height: 100px` und hoeherem Inhalt meldet den Inhalt — genau
    //   deswegen fragt eine Seite ueberhaupt.
    // * Am Wurzelelement ist es die Rollflaeche des DOKUMENTS, und die sagt
    //   das Layout (`Geometry::content`). Sie aus den Kaesten zu raten waere
    //   eine zweite Wahrheit ueber dieselbe Zahl: ein Hintergrund oder ein
    //   ueberlaufender Text haelt eine Seite rollbar, ohne einen Kasten zu
    //   haben.
    getter(&element_proto, "scrollHeight", |i, t, _| {
        if is_scrolling_root(i, &t) {
            let c = i.geometry.as_ref().map_or(0.0, |g| g.content.1 as f64);
            return Ok(Value::Num(c.max(viewport_f(i, "innerHeight"))));
        }
        Ok(Value::Num(scroll_area(i, &t).map_or(0.0, |(_, h)| h)))
    }, &fp);
    getter(&element_proto, "scrollWidth", |i, t, _| {
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
    // `offsetParent`: der naechste POSITIONIERTE Vorfahr, sonst der `<body>`
    // (CSSOM View §5). Die Ecke „positioniert" faehrt seit dieser Runde im
    // Layoutkasten mit — sie sonst zu beantworten hiesse, fuer jeden
    // Vorfahren die Kaskade neu aufzuloesen.
    //
    // `null` an einem Element ohne Kasten, am Wurzelelement und am `<body>`
    // selbst. Das ist die Antwort, auf die eine Seite prueft, wenn sie
    // fragt, ob ein Element ueberhaupt sichtbar ist.
    getter(&element_proto, "offsetParent", |i, t, _| {
        let id = node_of(i, &t)?;
        let (html, body) = match &i.doc {
            Some(d) => (d.html, d.body),
            None => return Ok(Value::Null),
        };
        if Some(id) == html || Some(id) == body { return Ok(Value::Null) }
        // Ohne Kasten gibt es keinen Bezug — `display: none`, und genau das
        // ist die uebliche Frage.
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
    // Rollen auf Verlangen. **Die Engine rollt NICHT** — sie merkt sich, was
    // die Seite wollte, und der Wirt holt es mit `take_scroll` ab. Dasselbe
    // Muster wie bei den Keksen und der Navigation: die Engine hat kein
    // Fenster und soll sich keines erfinden.
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
    // `scrollIntoView` rollt so weit, dass die OBERKANTE des Elements oben
    // steht — die Vorgabe der Spezifikation (`block: "start"`). Ein Argument
    // `false` oder `{ block: "end" }` stellt die Unterkante ans untere Ende.
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
    // Die Liste selbst arbeitet auf dem Element: sie haelt keine Kopie der
    // Klassen, sondern liest und schreibt das Attribut. Frisch je Zugriff —
    // `el.classList === el.classList` ist damit falsch, waehrend ein Browser
    // dasselbe Objekt liefert. Gemerkt, weil es eines Tages auffaellt.
    getter(&element_proto, "classList", |i, t, _| {
        let id = node_of(i, &t)?;
        let g = new_obj(Some(i.realm.token_list_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, &fp);
    // `el.style` ist eine SICHT auf das `style`-Attribut dieses Elements —
    // sie haelt keinen eigenen Zustand, also koennen Attribut und Sicht nicht
    // auseinanderlaufen.
    getter(&element_proto, "style", |i, t, _| {
        let id = node_of(i, &t)?;
        let g = new_obj(Some(i.realm.style_proto.clone()));
        g.borrow_mut().define(SLOT, Prop { value: Some(Value::Num(id as f64)), get: None,
            set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, &fp);

    // querySelector & Co. auf Element wie auf Document.
    // `append`, `prepend`, `before`, `after`, `replaceWith` — die moderne
    // Einhaengfamilie. Sie nimmt beliebig viele Argumente, und ein Text wird
    // dabei zum TEXTKNOTEN: `el.append("hallo")` haengt keinen String an.
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

    // Auch auf dem Bruchstueck: eine Schablone wird gefuellt, indem man in
    // ihrem Inhalt sucht — ohne das ist `.content` nur halb gebaut.
    for target in [&element_proto, &document_proto, &fragment_proto] {
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
    //
    // **Die drei lesen `this`, nicht nur das Hauptdokument.** Seit
    // `implementation.createHTMLDocument` gibt es einen ZWEITEN
    // Dokumentknoten im selben Feld, und ein Getter, der stur
    // `i.doc.body` zurueckgibt, haette dessen Rumpf ausgeliefert — jQuery
    // haette sein Probestueck in die ECHTE Seite geschrieben. Fuer das
    // Hauptdokument bleibt der gemerkte Weg; nur daneben wird gelaufen.
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
    // `scrollingElement` — das Element, dessen `scrollTop` die SEITE rollt.
    // Im Standardmodus ist das `documentElement`, und beak parst nichts
    // anderes. Eine Seite liest es und schreibt dann darauf; beide Antworten
    // muessen zueinander passen (`is_scrolling_root`).
    getter(&document_proto, "scrollingElement", |i, this, _| {
        match doc_part(i, &this, DocPart::Root)? { Some(x) => Ok(wrap(i, x)), None => Ok(Value::Null) }
    }, &fp);
    // **Vier Felder, die jede Seite liest — und die es bisher nicht gab.**
    // Im Zensus stehen `visibilityState`/`hidden` mit 50 und `referrer` mit
    // 45 Aufrufen (`docs/plan/WEB_PLATFORM_GAPS.md` P7). Ein FEHLENDES Feld
    // ist schlechter als eine richtige Antwort: `document.referrer.indexOf(…)`
    // stirbt auf `undefined`, und `if (document.hidden)` nimmt still den
    // falschen Zweig.
    //
    // beak malt genau ein Dokument, und es ist sichtbar, solange es laeuft —
    // das ist keine Hoeflichkeit, sondern der Zustand.
    getter(&document_proto, "visibilityState", |_, _, _| Ok(Value::str("visible")), &fp);
    getter(&document_proto, "hidden", |_, _, _| Ok(Value::Bool(false)), &fp);
    meth(&document_proto, "hasFocus", |_, _, _| Ok(Value::Bool(true)), 0, &fp);
    // `referrer` ist die LEERE Zeichenkette und keine erfundene Adresse: sie
    // ist die richtige Antwort fuer eine Navigation ohne Verweis, und beak
    // reicht bisher keinen weiter. Da statt fehlend — und wenn der Wirt ihn
    // einmal einreicht, steht die Stelle schon.
    getter(&document_proto, "referrer", |_, _, _| Ok(Value::str("")), &fp);
    // `document.cookie` — 1852 Aufrufe im Zensus, und auf BEIDEN Wikipedias
    // die erste Wand ueberhaupt: das allererste Inline-Skript jeder Seite
    // ruft `document.cookie.match(…)`, und auf `undefined` ist das das Ende
    // des Skripts.
    //
    // Die Engine haelt keinen Behaelter. Was dieses Dokument sehen darf,
    // haengt an Domain, Pfad, `Secure` und `HttpOnly` — das weiss der Wirt,
    // und `Interp::set_cookies` reicht ihm genau die Skript-Sicht ein.
    // Gesetztes geht denselben Weg zurueck (`take_cookie_sets`).
    accessor(&document_proto, "cookie",
        |i, _, _| { let c = i.cookies.clone(); Ok(Value::str(&c)) },
        |i, _, a| {
            let decl = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
            let Some((name, rest)) = decl.split_once('=') else { return Ok(Value::Undefined) };
            let name = name.trim().to_string();
            if name.is_empty() { return Ok(Value::Undefined) }
            let value = rest.split(';').next().unwrap_or("").trim().to_string();
            // Loeschen erkennt die Engine nur an `Max-Age<=0` — das ist
            // taktfrei. Ein `Expires` in der Vergangenheit braucht eine Uhr,
            // die sie nicht hat; DER Fall wird erst sichtbar, wenn der Wirt
            // die Sicht neu einreicht. Der Behaelter selbst hat beides.
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
    // Der Titel steht im Baum, nicht daneben: ein Skript, das ihn setzt,
    // aendert das `<title>`-Element, und wer ihn liest, liest denselben
    // Knoten. Zwei Kopien waeren zwei Wahrheiten.
    // `title` liest und schreibt am EIGENEN Dokumentknoten — sonst gaebe ein
    // frisch gebautes `createHTMLDocument("Titel")` den Titel der echten
    // Seite zurueck, und ein Setzer daran wuerde ihn ueberschreiben.
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
                // Kein `<title>`: eins anlegen und in den Kopf haengen. Ein
                // stiller Fehlschlag saehe aus wie ein kaputter Setzer.
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
    // Was der Aufrufzensus (`tools/jsscope/out/apicensus.json`, eine echte
    // Chromium-Messung auf denselben zwoelf Seiten) als naechstes verlangt.
    // Die Reihenfolge hier IST die Rangfolge dort — nicht die Reihenfolge,
    // in der mir etwas eingefallen ist.
    getter(&node_proto, "ownerDocument", |i, t, _| {          // 6081
        let id = node_of(i, &t)?;
        let Some(d) = &i.doc else { return Ok(Value::Null) };
        let root = d.doc;
        if id == root { return Ok(Value::Null) }              // das Dokument selbst: null
        Ok(wrap(i, root))
    }, &fp);
    meth(&node_proto, "getRootNode", |i, t, _| {              // 421
        let mut id = node_of(i, &t)?;
        loop {
            let Some(d) = &i.doc else { return Ok(Value::Null) };
            match d.nodes[id as usize].parent { Some(p) => id = p, None => break }
        }
        Ok(wrap(i, id))
    }, 0, &fp);
    getter(&element_proto, "namespaceURI", |i, t, _| {        // 3777
        // Nur die zwei, die vorkommen. Ein `foreignObject` in SVG bekaeme
        // hier die falsche Antwort — es kommt im Zielkorpus nicht vor, und
        // eine erfundene dritte waere schlimmer als eine ehrliche zweite.
        with_node!(i, t, |n| Ok(Value::str(
            if &*n.tag == "svg" || n.tag.starts_with("svg:") { "http://www.w3.org/2000/svg" }
            else { "http://www.w3.org/1999/xhtml" })))
    }, &fp);
    meth(&element_proto, "closest", |i, t, a| {               // 6115
        let sel = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut id = Some(node_of(i, &t)?);
        while let Some(x) = id {
            let hit = i.doc.as_ref().is_some_and(|d| selector_match(d, x, &sel));
            if hit { return Ok(wrap(i, x)) }
            let Some(d) = &i.doc else { break };
            id = d.nodes[x as usize].parent;
        }
        Ok(Value::Null)
    }, 1, &fp);
    meth(&element_proto, "getAttributeNames", |i, t, _| {     // 165
        let names: Vec<Value> = with_node!(i, t, |n|
            n.attrs.iter().map(|(k, _)| Value::str(k)).collect::<Vec<_>>());
        Ok(i.new_array(names))
    }, 0, &fp);
    meth(&element_proto, "hasAttributes", |i, t, _| {         // 159
        with_node!(i, t, |n| Ok(Value::Bool(!n.attrs.is_empty())))
    }, 0, &fp);
    getter(&element_proto, "dataset", |i, t, _| {             // 3966
        // Eine MOMENTAUFNAHME, kein lebendes Objekt: `el.dataset.x = 1`
        // schreibt damit KEIN Attribut. Das ist eine echte Luecke und hier
        // benannt statt versteckt — der Zensus zaehlt fast nur Lesezugriffe.
        let pairs: Vec<(String, String)> = with_node!(i, t, |n|
            n.attrs.iter().filter_map(|(k, v)| k.strip_prefix("data-")
                .map(|r| (dash_to_camel(r), v.to_string()))).collect::<Vec<_>>());
        let g = new_obj(Some(i.realm.object_proto.clone()));
        for (k, v) in pairs { g.borrow_mut().define(&k, Prop::data(Value::string(v))); }
        Ok(Value::Obj(g))
    }, &fp);
    getter(&document_proto, "defaultView", |i, _, _| {        // 1580
        Ok(Value::Obj(i.realm.global.clone()))
    }, &fp);
    // `document.forms` — die Sammlung, ueber die Seiten ihr Formular finden.
    //
    // Mit BENANNTEM Zugriff: `document.forms["loginForm"]` sucht ueber `id`
    // UND `name`, und genau diese Form nehmen Seiten. Eine Liste ohne
    // Namenszugriff waere die halbe Sache — sie gibt `undefined` und sagt
    // nicht, warum.
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
    getter(&document_proto, "activeElement", |i, _, _| {      // 1606
        // Was `focus()` gesetzt hat — sonst `body`, die Antwort, die ein
        // Browser ohne Fokus auch gibt.
        if let Some(f) = i.doc.as_ref().and_then(|d| d.focused) { return Ok(wrap(i, f)) }
        let b = i.doc.as_ref().and_then(|d| find_tag(d, "body"));
        Ok(match b { Some(x) => wrap(i, x), None => Value::Null })
    }, &fp);
    meth(&document_proto, "createComment", |i, _, a| {        // 2398
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(COMMENT_NODE, "#comment");
        d.nodes[id as usize].text = s;
        Ok(wrap(i, id))
    }, 1, &fp);
    meth(&document_proto, "createElementNS", |i, _, a| {      // 180
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
        let lower = s.to_lowercase();
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &lower);
        Ok(wrap(i, id))
    }, 1, &fp);
    // **`new Image()` ist `document.createElement("img")`** — dieselbe
    // Sache, ein anderer Name. Ohne den Konstruktor bricht der Zeitgeber der
    // Google-Ergebnisseite mit `Image is not defined` ab; ein Zaehlpixel ist
    // der haeufigste Gebrauch, und der besteht genau aus `new Image().src =
    // …`. Gebaut wird deshalb ein ECHTES `img`-Element und kein Attrappe:
    // was die Seite danach daran tut, tut sie an einem Knoten im Baum.
    let img_ctor = native(Some(fp.clone()), |i, _, a| {
        let node = {
            let Some(d) = &mut i.doc else { return i.type_err("no document") };
            d.create(ELEMENT_NODE, "img")
        };
        let el = wrap(i, node);
        // `new Image(w, h)` setzt Breite und Hoehe als ATTRIBUTE, so wie im
        // Browser — nicht als Stil.
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
    // `importNode` (2134 Aufrufe) und `adoptNode`: beide holen einen Knoten
    // in DIESES Dokument. beak hat genau eins — es gibt keinen zweiten Baum,
    // aus dem etwas kaeme —, also ist `importNode` eine Kopie und `adoptNode`
    // der Knoten selbst. Das ist keine Abkuerzung, sondern was die
    // Spezifikation fuer den Ein-Dokument-Fall sagt.
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
    // **Warum es das braucht.** Alpine, htmx und jede Bibliothek, die
    // nachgeladenes HTML von selbst zum Leben erweckt, meldet sich beim BAUM
    // an statt bei einem Ereignis. Ohne `MutationObserver` stirbt Alpine
    // schon beim Laden — `ReferenceError`, ausserhalb jedes `try`.
    //
    // Aufgezeichnet wird im Baum (`Doc::record`), zugestellt am
    // Microtask-Kontrollpunkt (`promise::run_jobs`). Die Meldungen werden
    // erst beim Zustellen GEBAUT: ein fertiges Objekt je Aenderung, waehrend
    // das Skript laeuft, waere Arbeit fuer einen Leser, den es vielleicht
    // nie gibt.
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
        // `attributeFilter` schaltet `attributes` MIT ein, auch wenn niemand
        // es hingeschrieben hat — so steht es in der Spezifikation, und
        // Bibliothekscode verlaesst sich darauf.
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
        // Ohne eines der drei ist nichts zu beobachten — die Spezifikation
        // wirft hier, statt still einen Beobachter anzulegen, der nie meldet.
        if !child_list && !attrs && !char_data {
            return i.type_err("MutationObserver.observe: childList, attributes or characterData required");
        }
        let Value::Obj(this) = &t else { return i.type_err("MutationObserver.observe: not an observer") };
        let reg = MutReg { target, slot: usize::MAX, subtree, child_list, attrs,
                           attr_old, char_data, char_old, filter };
        let mut found = false;
        for o in i.observers.iter_mut() {
            if Rc::ptr_eq(&o.js, this) {
                // Ein zweites `observe` auf DENSELBEN Knoten ersetzt die
                // Anmeldung, es haengt keine zweite an (DOM §4.3.1).
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
        // Erst einsammeln, was der Baum seit dem letzten Mal notiert hat —
        // sonst gaebe `takeRecords()` unmittelbar nach einer Aenderung eine
        // leere Liste, und genau dafuer ruft man es.
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
    // **Warum es das braucht.** Jedes Diagramm, jede Karte und jede
    // Bibliothek, die sich an ihren Kasten anpasst, meldet sich hier an statt
    // am `resize` des Fensters — der sagt nichts darueber, dass sich EIN
    // Kasten geaendert hat, weil daneben etwas eingeklappt wurde.
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
        // `{ box: "border-box" }` — die Vorgabe ist der Inhaltskasten.
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
                // Ein zweites `observe` auf DENSELBEN Knoten ersetzt die
                // Anmeldung (Resize Observer 3.1) — und `last: None` sorgt
                // dafuer, dass es DANACH einmal meldet, so wie beim ersten
                // Mal.
                o.regs.retain(|r| r.target != target);
                o.regs.push(ResizeReg { target, kind, last: None });
                found = true;
                break;
            }
        }
        if !found { return i.type_err("ResizeObserver.observe: not an observer") }
        // Sofort auswerten: `observe` liefert die aktuelle Groesse, nicht
        // erst die naechste Aenderung. Wer bis zum naechsten Layout wartet,
        // laesst eine Seite ohne Groesse dastehen, die sich nie mehr aendert.
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
    // **Warum es das braucht.** Verzoegert geladene Bilder, unendliche
    // Listen, „im Blick"-Animationen und jede Statistik, die zaehlt, was
    // gesehen wurde. Ohne ihn laedt eine Bildergalerie genau ein Bild und
    // haelt dann an — nicht mit einem Fehler, sondern mit Ruhe, und das ist
    // schlimmer.
    let io_proto = new_obj(Some(realm.object_proto.clone()));
    let io_ctor = native(Some(fp.clone()), |i, _, a| {
        let cb = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&cb) {
            return i.type_err("IntersectionObserver: argument 1 is not a function");
        }
        let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
        let has = matches!(opts, Value::Obj(_));
        // Die Wurzel: ein Element, oder das Sichtfeld.
        let root = if !has { None } else {
            match i.get(&opts, "root")? {
                v @ Value::Obj(_) => node_of(i, &v).ok(),
                _ => None,
            }
        };
        // `rootMargin` — die CSS-Kurzform mit ein bis vier Laengen. Prozente
        // stehen zur AUSSCHNITTgroesse; ohne eigene Wurzel ist das das
        // Sichtfeld.
        let (vw, vh) = i.viewport;
        let margin_src: Rc<str> = if !has { Rc::from("0px") } else {
            match i.get(&opts, "rootMargin")? {
                Value::Str(s) => s.clone(),
                Value::Undefined => Rc::from("0px"),
                v => i.to_string(&v)?,
            }
        };
        let margin = parse_root_margin(&margin_src, vw, vh);
        // `threshold` — eine Zahl oder eine Liste. Ohne Angabe: 0, also
        // „sobald ein Pixel sichtbar wird".
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
        // Wie beim `ResizeObserver`: die erste Meldung kommt sofort, nicht
        // erst beim naechsten Bildlauf. Genau darauf verlaesst sich jede
        // Liste, die beim Laden schon halb sichtbar ist.
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
    // `root`, `rootMargin` und `thresholds` sind lesbar — Bibliothekscode
    // liest sie zurueck, um einen Beobachter wiederzuverwenden.
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
    // **`createHTMLDocument` ist der Grund, warum es das hier gibt.** jQuery
    // baut damit ein WEGWERF-Dokument, um fremdes HTML zu zerlegen, ohne
    // dass dabei Bilder geladen oder Skripte gefahren werden — und es tut
    // das schon beim Laden, ausserhalb jedes `try`. Ohne diese Zeile stirbt
    // jQuery an seiner eigenen Merkmalspruefung, und mit ihm die halbe
    // Bibliothek der Seite.
    //
    // Ein zweiter Dokumentknoten IM SELBEN Knotenfeld, nicht ein zweites
    // Feld: die Knoten-ids sind Plaetze in genau einem `Vec`, und ein
    // zweiter Baum daneben waere ein zweites Adressraum-Modell. Losgeloest
    // ist er trotzdem — er haengt an keinem Elter, also sieht ihn weder das
    // Layout noch der Wirt.
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
        // `createHTMLDocument()` OHNE Argument bekommt keinen Titel — das
        // ist etwas anderes als der leere Titel, den `("")` verlangt.
        if let Some(t) = title {
            let el = d.create(ELEMENT_NODE, "title");
            let tx = d.create(TEXT_NODE, "#text");
            d.nodes[tx as usize].text = t;
            d.append(el, tx);
            d.append(head, el);
        }
        Ok(wrap(i, doc))
    }, 1, &fp);
    // `hasFeature` sagt laut Spezifikation IMMER true — sie ist Altlast und
    // ausdruecklich so festgeschrieben, damit niemand mehr danach fragt.
    meth(&impl_obj, "hasFeature", |_, _, _| Ok(Value::Bool(true)), 0, &fp);
    // `createDocument` ist der XML-Zwilling: ein Dokumentknoten mit genau
    // einem Wurzelelement, kein `head`, kein `body`. Der Namensraum wird
    // GELESEN und fallengelassen — beaks Baum kennt keine Namensraeume, und
    // ein erfundener waere schlimmer als keiner.
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

    // Ein Dokument IST ein `XPathEvaluator` (DOM 4 §XPathEvaluatorBase), und
    // `document.evaluate(...)` ist der Einstieg, den eine Seite schreibt —
    // `new XPathEvaluator()` ist der von Bibliotheken.
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

    // ── Die Schnittstellen als globale Konstruktoren ─────────────────────
    //
    // Nicht Zierde: `el instanceof HTMLLinkElement` und
    // `class X extends HTMLElement` sind auf DREI der elf Zielseiten die
    // ERSTE Wand — vor jeder Sprachluecke. Gezaehlt, nicht vermutet
    // (`wallcheck WCPAGE=*`).
    //
    // Die Kette ist die echte: EventTarget -> Node -> Element -> HTMLElement
    // -> HTMLxyzElement. Eine flache Liste taete es fuer `instanceof
    // HTMLElement` auch, aber dann waere `link instanceof Element` falsch —
    // und genau solche Ketten fragt Bibliothekscode ab.
    let html_element_proto = new_obj(Some(element_proto.clone()));
    let svg_element_proto = new_obj(Some(element_proto.clone()));

    // `new HTMLElement()` wirft — so wie im Browser. Die KLASSENDEFINITION
    // `class X extends HTMLElement {}` laeuft trotzdem durch: sie liest nur
    // `HTMLElement.prototype`, gerufen wird der Konstruktor erst bei `new`.
    fn iface(realm: &Realm, name: &str, proto: &Gc) -> Gc {
        let c = native(Some(realm.function_proto.clone()),
                       |i, _, _| i.type_err("Illegal constructor"), name, 0, true);
        c.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
        proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c.clone())));
        // **`Symbol.toStringTag` traegt den Schnittstellennamen** (WebIDL
        // 3.7.3). Ohne ihn meldete `Object.prototype.toString.call(el)`
        // `[object Object]` — und genau daran erkennt Bibliothekscode ein
        // GEWOEHNLICHES Objekt: jQuerys `isPlainObject` hielt jeden Knoten
        // fuer eine Datenstruktur und stieg beim tiefen Kopieren ueber
        // `parentNode` in einen Ring, aus dem es keinen Ausgang gibt.
        //
        // Hier und nicht an 60 Stellen: `iface` ist der EINE Weg, auf dem
        // eine DOM-Schnittstelle entsteht.
        proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str(name)));
        realm.global.borrow_mut().define(name, Prop::builtin(Value::Obj(c.clone())));
        c
    }
    iface(realm, "EventTarget", &event_target_proto);
    // Das Fenster IST ein EventTarget — dadurch hat `window` dieselben drei
    // Methoden wie jeder Knoten, ohne sie ein zweites Mal zu definieren.
    realm.global.borrow_mut().proto = Some(event_target_proto.clone());
    // Und das Fenster heisst `Window`, nicht `EventTarget`. Ohne diese Zeile
    // erbt es die Marke seines Prototyps, und `toString.call(window)` sagt
    // etwas Falsches statt gar nichts. KEIN `iface`: das legte ein
    // `constructor` auf das globale Objekt, und dort steht schon alles, was
    // die Seite selbst definiert.
    realm.global.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("Window")));
    let node_ctor = iface(realm, "Node", &node_proto);
    // **Die Knotentyp-Konstanten.** Sie stehen laut Spezifikation auf dem
    // Konstruktor UND auf dem Prototyp. Ohne sie ist `Node.ELEMENT_NODE`
    // `undefined`, und ein `switch(el.nodeType){case Node.ELEMENT_NODE: …}`
    // faellt still in den `default`-Zweig: die Fritzbox-Oberflaeche hat
    // damit ihre GANZE Anmeldemaske gebaut und dann nicht angehaengt — kein
    // Fehler, keine Meldung, nur ein leeres `<body>`.
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
    // NACH `iface`: die legt einen „Illegal constructor" auf denselben
    // Prototyp, und wer zuletzt schreibt, gewinnt.
    iface(realm, "HTMLElement", &html_element_proto);
    install_custom_elements(realm, &html_element_proto);
    iface(realm, "SVGElement", &svg_element_proto);
    // `CharacterData` sitzt zwischen Node und Text — 453 Aufrufe im Zensus
    // fragen `.data`, und die Kette ist die, die Bibliothekscode abfragt.
    let char_data_proto = new_obj(Some(node_proto.clone()));
    iface(realm, "CharacterData", &char_data_proto);
    text_proto.borrow_mut().proto = Some(char_data_proto.clone());
    iface(realm, "Text", &text_proto);
    // Ein Kommentar ist KEIN HTMLElement — vorher landete er dort, weil
    // `wrap` ihn wie ein unbekanntes Tag behandelte.
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
    // `el.attributes` (40 Aufrufe), `NamedNodeMap.length` (35) und die drei
    // `Attr`-Felder (72). Zusammen 147, und sie haengen aneinander: ohne
    // `Attr` ist die Karte leer, ohne die Karte ist `attributes` nutzlos.
    //
    // Die Karte ist eine MOMENTAUFNAHME, keine lebende Sicht. Ein Browser
    // gibt eine lebende; wer die Karte haelt und dazwischen ein Attribut
    // setzt, saehe hier den alten Stand. Gemerkt, weil es eines Tages
    // auffaellt — die 147 Aufrufe lesen alle sofort.
    //
    // Die Felder sitzen auf dem PROTOTYP, nicht auf jedem Gegenstand — so
    // wie im Browser. Sie auf die Instanz zu legen waere kuerzer und fuer
    // `attributes[0].name` nicht zu unterscheiden; es faellt erst auf, wenn
    // jemand `Attr.prototype` befragt, und genau das tut die Lueckenprobe.
    let attr_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "Attr", &attr_proto);
    /// Ein verdecktes Feld lesen. Eingebaute Funktionen sind Zeiger und
    /// fangen nichts ein — also steht der Feldname im Rumpf.
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
    // Wir fuehren keine Namensraeume. `null` ist die richtige Antwort fuer
    // HTML-Attribute, nicht eine fehlende.
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
            // Auch unter dem NAMEN: `el.attributes.href` ist die uebliche
            // Schreibweise, und die Spezifikation kennt sie (WebIDL
            // `[LegacyUnenumerableNamedProperties]`).
            m.define(k, Prop::data(Value::Obj(a)));
        }
        map.borrow_mut().define("__len", Prop { value: Some(Value::Num(attrs.len() as f64)),
            get: None, set: None, writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(map))
    }, &fp);
    // `toggleAttribute(name, force?)` — 56 Aufrufe. Es liefert, ob das
    // Attribut DANACH da ist, und darauf verlaesst sich der uebliche Einzeiler
    // `el.setAttribute("aria-expanded", el.toggleAttribute("open"))`.
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

    // ── Der Kleinkram aus P7 ─────────────────────────────────────────────
    //
    // Je Zeile fuenf Zeilen Arbeit, zusammen rund 400 Aufrufe im Zensus. Sie
    // stehen hier zusammen, weil sie EINE Sache gemeinsam haben: jede war
    // nicht falsch, sondern GAR NICHT da — und ein fehlendes Feld ist ein
    // `TypeError` mitten in fremdem Code, kein falscher Wert, den man sieht.

    // `nextElementSibling`/`previousElementSibling` gehoeren zu
    // `NonDocumentTypeChildNode` — also an Element UND an Text/Kommentar
    // (112 + 54 Aufrufe). Der Zensus nennt beide getrennt, und beide gibt es.
    for proto in [&element_proto, &char_data_proto] {
        getter(proto, "nextElementSibling",
               |i, t, _| element_sibling(i, &t, 1), &fp);
        getter(proto, "previousElementSibling",
               |i, t, _| element_sibling(i, &t, -1), &fp);
        // `remove()` gehoert zu `ChildNode`, also an BEIDE — es sass nur auf
        // Element, und `CharacterData.remove` steht mit 18 Aufrufen im
        // Zensus. Ein Textknoten, den man nicht loswird, ist genau die Sorte
        // Luecke, die man erst am fremden Code merkt.
        meth(proto, "remove", |i, t, _| {
            let id = node_of(i, &t)?;
            if let Some(d) = &mut i.doc { d.detach(id); }
            Ok(Value::Undefined)
        }, 0, &fp);
    }
    // `lastElementChild`/`childElementCount` — die Geschwister von
    // `firstElementChild`, das es schon gab. Sie einzeln nachzureichen, wenn
    // sie das naechste Mal fehlen, waere dreimal derselbe Weg.
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
        // `replaceChildren(...)` — alles raus, das Neue rein. 52 Aufrufe, und
        // es ist die moderne Schreibweise fuer `innerHTML = ""` plus
        // anhaengen; wer sie nicht hat, bekommt eine halb geleerte Liste.
        meth(proto, "replaceChildren", |i, t, a| {
            let id = node_of(i, &t)?;
            let alt: Vec<u32> = i.doc.as_ref()
                .map(|d| d.nodes[id as usize].children.clone()).unwrap_or_default();
            for c in alt { if let Some(d) = &mut i.doc { d.detach(c); } }
            insert_all(i, &t, a, Where::Last)
        }, 0, &fp);
    }
    // Auf `document` und `<html>` fehlten `firstElementChild`/`children`
    // ebenfalls — sie sitzen bisher nur auf Element.
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
    // `isConnected` (42): haengt dieser Knoten am Dokument? Genau diese Frage
    // stellt jede Bibliothek, bevor sie an einem Knoten misst — ein Knoten
    // ausserhalb des Baumes hat keinen Kasten, und ihn zu messen liefert
    // Nullen, die aussehen wie eine Messung.
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
    // `classList` gab es; was fehlte, war der Name — 1381 Aufrufe, und die
    // Methoden sassen auf JEDER Liste einzeln statt auf einem Prototyp.
    let token_list_proto = new_obj(Some(realm.object_proto.clone()));
    iface(realm, "DOMTokenList", &token_list_proto);
    /// Die Klassen eines Knotens schreiben — eine Stelle, ein Format.
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
        // `toggle(name, kraft)` — das zweite Argument entscheidet statt des
        // Zustands, und Seiten benutzen es fuer „setze genau so".
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
    // Vorher gab `el.style` bei JEDEM Zugriff ein frisches leeres Objekt:
    // ein Schreibzugriff verschwand, und ein Lesen danach fand nichts. Das
    // war als ehrlicher Stumpf gemeint, ist aber der haeufigste Eingriff
    // ueberhaupt — `el.style.display = "none"` ist Zeigen und Verstecken.
    //
    // Jetzt ist es eine SICHT auf das `style`-Attribut. Damit wirkt die
    // Zuweisung wirklich: die Kaskade liest dasselbe Attribut, und `dirty`
    // sagt beak, dass neu ausgelegt werden muss.
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
    // Die benannten Eigenschaften. Die Liste ist bewusst endlich: ohne Proxy
    // gibt es keinen Weg, JEDEN Namen abzufangen, und eine Liste, die die
    // gebraeuchlichen deckt, ist besser als ein Stumpf, der keinen deckt.
    // Was nicht daraufsteht, geht ueber `setProperty`/`getPropertyValue`.
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
    // **`ShadowRoot` gibt es als SCHNITTSTELLE, auch ohne Shadow DOM.**
    // htmx fragt `e.parentNode instanceof ShadowRoot`, um einen Elter durch
    // eine Schattengrenze zu finden — und ein `instanceof` gegen einen
    // fehlenden Namen ist ein `ReferenceError`, der die ganze Bibliothek
    // umbringt, statt `false` zu ergeben.
    //
    // `false` IST hier die wahre Antwort: beak haengt nirgends einen
    // Schattenbaum an, also ist der Elter eines Knotens nie einer. So sieht
    // ein Browser auf jeder Seite aus, die `attachShadow` nie ruft — das
    // Schnittstellenobjekt steht da, eine Instanz gibt es nicht. Shadow DOM
    // selbst bleibt gemessen kein Ziel
    // ([[feedback_a_call_count_is_not_a_site_count]]).
    let shadow_root_proto = new_obj(Some(fragment_proto.clone()));
    iface(realm, "ShadowRoot", &shadow_root_proto);

    // ── Event ────────────────────────────────────────────────────────────
    //
    // Das Ereignisobjekt gab es schon — als flache Huelle mit Datenfeldern.
    // Was fehlte, war der NAME: `e instanceof Event` scheitert daran, nicht
    // an `e.target`, und im Zensus haengen 1320 Aufrufe daran.
    //
    // Die Felder liegen jetzt in Schlitzen und die Prototypen lesen sie —
    // sonst stuende `target` auf der INSTANZ und `Event.prototype.target`
    // waere trotzdem leer, also genau die Abfrage, die scheitert.
    let event_proto = new_obj(Some(realm.object_proto.clone()));
    let fp2 = realm.function_proto.clone();
    // Ein eingebautes Getter je Feld. Ein Funktionszeiger faengt nichts ein,
    // also traegt jedes seinen Schlitznamen im Rumpf — das Makro schreibt sie.
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
        // Nur ein abbrechbares Ereignis laesst sich abbrechen — sonst meldet
        // `defaultPrevented` einen Halt, den niemand beachtet.
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
    meth(&event_proto, "composedPath", |i, t, _| {
        let tgt = i.get(&t, EV_TARGET)?;
        let Ok(id) = node_of(i, &tgt) else { return Ok(i.new_array(Vec::new())) };
        let chain = ancestors(i, id);
        // Vom Ziel nach aussen — `ancestors` liefert die Zustellreihenfolge,
        // also aussen zuerst.
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

    // `CustomEvent` ist ein `Event` mit einem Feld — und mit 412 Aufrufen die
    // Art, in der Seiten untereinander reden.
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
    custom_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(custom_proto.clone())));
    custom_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(custom_ctor.clone())));
    custom_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CustomEvent")));
    realm.global.borrow_mut().define("CustomEvent", Prop::builtin(Value::Obj(custom_ctor)));
    // `PromiseRejectionEvent` — die Art, unter der eine unbehandelte
    // Ablehnung ans Fenster kommt.
    //
    // Die Marke ist keine leere Zusage: beak meldet unbehandelte Ablehnungen
    // wirklich (`promise::report_rejections` schickt genau dieses Ereignis
    // ans Fenster und schreibt danach auf die Konsole). Sie zu HABEN ist
    // zugleich das, woran core-js erkennt, ob die Umgebung Versprechen
    // browsermaessig behandelt — fiel die Pruefung durch, ersetzte es die
    // eingebaute `Promise` durch seine eigene, und die kennt in der Fassung,
    // die die Fritzbox ausliefert, kein `allSettled`.
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

    // `getComputedStyle` — 443 Aufrufe im Zensus.
    //
    // Seit 0.64.0 antwortet es aus der KASKADE: der Wirt reicht mit
    // `set_style_context` Blatt, Baum, Thema und Fensterbreite ein, und
    // `computed_decls` rechnet damit dieselbe Kaskade wie das Layout — auf
    // demselben Baum, mit demselben Blatt. Die Werte kommen in
    // CSSOM-Schreibweise heraus (`rgb(0, 0, 0)`, nicht `#000`), also NICHT
    // in der des Autors.
    //
    // Ohne Kontext bleibt es beim Inline-Stil. Das ist eine Teilantwort, aber
    // die Funktion ganz wegzulassen hiesse TypeError, und ein TypeError
    // beendet das Skript.
    // Der Rollstand am Fenster. **Er stand fest auf 0** — `set_viewport` hat
    // ihn als Zahl abgelegt, und danach hat ihn nie jemand nachgezogen. Eine
    // Seite, die `window.scrollY` liest, um zu entscheiden, ob die Kopfzeile
    // kleben soll, bekam ueberall die Antwort „ganz oben".
    //
    // Jetzt Zugriffsfunktionen auf die Geometrie: dieselbe Quelle, aus der
    // `getBoundingClientRect` rechnet, und damit dieselbe Wahrheit.
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
        // Der gerechnete Stil, wenn der Wirt einen Kaskadenkontext eingereicht
        // hat. Ein SCHNAPPSCHUSS, kein lebender Verweis — genau das ist
        // `getComputedStyle` auch im Browser: die Antwort auf die Frage von
        // jetzt. Ohne Kontext bleibt es beim Inline-Stil, und das ist eine
        // Teilantwort, die die Seite laufen laesst.
        // Gerechnet wird auf dem LEBENDEN Baum, und die Kennung ist der
        // Arena-Index — dieselbe Zahl, die das JS-Objekt haelt. Ein Element,
        // das noch nirgends haengt, findet `find_path` nicht: dafuer gibt es
        // keinen gerechneten Stil, und die leere Antwort ist die ehrliche.
        if let Some(text) = computed_decls(i, id) {
            g.borrow_mut().define(COMPUTED, Prop { value: Some(Value::str(&text)), get: None,
                set: None, writable: false, enumerable: false, configurable: false });
        }
        Ok(Value::Obj(g))
    }, 1, &fp);

    // ── Behandler als Eigenschaft ────────────────────────────────────────
    //
    // `el.onclick = f` — 645 Aufrufe im Zensus, und bis hierher gab es davon
    // NUR die Attributform. Eine Seite, die den Behandler zuweist statt ihn
    // ins HTML zu schreiben, hatte gar keinen.
    //
    // Zugestellt wird trotzdem nur, was in `DISPATCHED` steht (heute:
    // `click`). Die uebrigen Namen anzunehmen ist kein Vortaeuschen — die
    // Anmeldung DARF nicht werfen, sonst stirbt die Seite an einer Zeile,
    // die im Browser auch nichts tut, solange nichts passiert.
    handler_prop!(html_element_proto, fp, "onclick", "click");
    handler_prop!(html_element_proto, fp, "onload", "load");
    handler_prop!(html_element_proto, fp, "onerror", "error");
    handler_prop!(html_element_proto, fp, "onchange", "change");
    handler_prop!(html_element_proto, fp, "oninput", "input");
    handler_prop!(html_element_proto, fp, "onsubmit", "submit");
    // `focus()` / `blur()` — die Seite bestimmt, wo die Tastatur hinschreibt.
    //
    // Der Fokus steht im Dokument, nicht in einem Nebenzustand: `document
    // .activeElement` liest dieselbe Stelle, und der Wirt uebernimmt sie in
    // seinen eigenen (`forms.rs`). Die Ereignisse werden dabei WIRKLICH
    // zugestellt — ein `focus()`, das nur einen Wert setzt, waere fuer eine
    // Seite mit `onfocus` unsichtbar.
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
    // Die Zeigerereignisse — 150 Aufrufe im Zensus. Zugestellt wird davon
    // heute nichts (beak kennt `click`), aber die ANMELDUNG darf nicht
    // werfen: eine Seite, die `el.onpointermove = f` schreibt, stirbt sonst
    // an einer Zeile, die im Browser auch nichts tut, solange der Zeiger
    // stillsteht.
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

    // Dieselben auf dem Fenster: `window.onload = …` ist die aelteste
    // Schreibweise ueberhaupt und steht auf fast jeder alten Seite.
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

    // ── Felder, die auf einem Attribut sitzen ────────────────────────────
    //
    // Alle aus dem Zensus, keins geraten. Sie sind billig, weil das Attribut
    // schon da ist — was fehlte, war der NAME, unter dem Seiten es abfragen.
    attr_prop!(html_element_proto, fp, "title", "title");
    attr_prop!(html_element_proto, fp, "lang", "lang");
    attr_prop!(html_element_proto, fp, "dir", "dir");
    attr_prop!(html_element_proto, fp, "accessKey", "accesskey");
    attr_prop!(html_element_proto, fp, "contentEditable", "contenteditable");
    bool_attr_prop!(html_element_proto, fp, "hidden", "hidden");
    // `tabIndex` ist -1, wenn nichts dasteht: „nicht mit der Tabtaste
    // erreichbar". Eine 0 hiesse das Gegenteil.
    num_attr_prop!(html_element_proto, fp, "tabIndex", "tabindex", -1.0);
    attr_prop!(element_proto, fp, "slot", "slot");
    // ── ARIA-Spiegelung ──────────────────────────────────────────────────
    //
    // `ariaHidden` steht mit 68 Aufrufen im Zensus — tagesschau.de liest es
    // bei jedem Umschalten. Die Nachbarn kommen mit, weil sie dasselbe Muster
    // haben und weil die naechste Seite eines davon liest: sie einzeln
    // nachzureichen waere achtmal derselbe Weg.
    //
    // `role` ist die Ausnahme in der Liste: es heisst auch im Attribut so.
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
    // Je Schnittstelle das, was auf ihr wirklich abgefragt wird — aus dem
    // Aufrufzensus, nicht aus der Spezifikation: `HTMLAnchorElement.href`
    // steht dort mit 285 Aufrufen, `HTMLScriptElement.src` mit 144. Was
    // niemand ruft, steht hier nicht.
    //
    // **`href` und `src` sind ROH**, so wie sie im Attribut stehen. Im
    // Browser sind sie AUFGELOEST — `a.href` einer relativen Adresse ist die
    // absolute. Das braucht die Adresse der Seite und ist eine eigene Zeile;
    // bis dahin ist `.href` dasselbe wie `getAttribute("href")`, und das ist
    // nachpruefbar falsch statt still falsch.
    // HTMLAnchorElement
    if let Some(p) = tag_protos.get("a") {
        attr_prop!(p, fp, "href", "href");
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "target", "target");
        attr_prop!(p, fp, "rel", "rel");
        attr_prop!(p, fp, "download", "download");
        attr_prop!(p, fp, "hreflang", "hreflang");
        // `a.hash` — 20 Aufrufe, und es ist das Stueck, an dem eine Seite
        // erkennt, ob ein Verweis auf denselben Abschnitt zeigt. Aus dem
        // ROHEN `href`, wie die Nachbarn auch: `href` ist hier nicht
        // aufgeloest, und `hash` daraus aufzuloesen waere die eine Zeile, die
        // aus der Reihe tanzt.
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
        // `currentSrc` (28 Aufrufe): WELCHE Quelle wurde wirklich genommen.
        // beak waehlt aus `srcset` im Layout (`picture.rs`), und die Engine
        // kennt diese Wahl nicht — sie kennt nur das Dokument. Also `src`,
        // und das ist die richtige Antwort fuer jedes Bild ohne `srcset`;
        // fuer eines MIT ist es die Quelle, die im Markup steht, und nicht
        // die leere Zeichenkette, an der Wikipedias Bildcode heute abbricht.
        getter(p, "currentSrc", |i, t, _| {
            with_node!(i, t, |n| Ok(match n.attr("src") {
                Some(v) => Value::Str(v.clone()), None => Value::str("") }))
        }, &fp);
    }
    // HTMLInputElement
    if let Some(p) = tag_protos.get("input") {
        attr_prop!(p, fp, "type", "type");
        attr_prop!(p, fp, "name", "name");
        attr_prop!(p, fp, "placeholder", "placeholder");
    }
    // ── Der WERT eines Steuerelements ────────────────────────────────────
    //
    // `value` ist der schmutzige Wert, `defaultValue` das Attribut — die
    // Spezifikation trennt beide, und ein Browser aendert beim Setzen von
    // `.value` das Attribut nicht. Bis hierher war `input.value` eine
    // gewoehnliche Eigenschaft auf der HUELLE: sie las sich zurueck und
    // erreichte weder Baum noch Layout noch das abgeschickte Formular.
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
        }
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
        // `submit()` schickt OHNE `submit`-Ereignis ab — das ist der
        // Unterschied zu `requestSubmit()`, und Seiten verlassen sich darauf
        // (ihr eigener `onsubmit` soll nicht ein zweites Mal laufen).
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
    // Ein `<select>` hatte weder `value` noch `options` noch `selectedIndex`.
    // Auf der Fritzbox-Anmeldeseite stirbt daran der Aufbau des
    // Anmeldeformulars — `gUsernameElem.value.length` liest `.length` von
    // `undefined`, und der Fehler nennt weder Element noch Zeile.
    //
    // Die Wahrheit ist der BAUM, nicht ein Nebenzustand: `selected` ist das
    // Attribut, `value` faellt auf den Text zurueck. Genau so liest das
    // Layout die Auswahl (`forms.rs::collect_options`), also koennen die
    // beiden Seiten nicht auseinanderlaufen.
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

    // `<template>.content` — 2245 Aufrufe, die groesste einzelne Luecke im
    // Zensus. Der Inhalt einer Schablone gehoert laut Spezifikation NICHT in
    // den Baum, sondern in ein eigenes Bruchstueck.
    //
    // Umgehaengt wird erst beim ersten Zugriff. Wer nie `.content` liest,
    // behaelt die Kinder im Baum, und `to_dom` schreibt sie zurueck wie
    // bisher — gemalt werden sie ohnehin nicht (`style.rs` gibt `<template>`
    // kein Kaestchen). Das ist der billige Weg zu spec-treuem Verhalten,
    // ohne den Weg zurueck ins Layout anzufassen.
    if let Some(tpl) = tag_protos.get("template") {
        getter(tpl, "content", |i, t, _| {
            let id = node_of(i, &t)?;
            if let Some(f) = i.doc.as_ref().and_then(|d| d.nodes[id as usize].content) {
                return Ok(wrap(i, f));
            }
            let Some(d) = &mut i.doc else { return i.type_err("no document") };
            let f = d.create(ELEMENT_NODE, "#fragment");
            for k in d.nodes[id as usize].children.clone() { d.append(f, k); }
            d.nodes[id as usize].content = Some(f);
            Ok(wrap(i, f))
        }, &fp);
    }
    // SVG kennt genau eine Unterscheidung, die Seiten wirklich abfragen.
    {
        let p = new_obj(Some(svg_element_proto.clone()));
        iface(realm, "SVGSVGElement", &p);
        tag_protos.insert("svg", p);
    }

    install_text_codec(realm);
    install_xpath(realm);

    realm.node_proto = node_proto;
    realm.element_proto = element_proto;
    realm.text_proto = text_proto;
    realm.document_proto = document_proto;
    realm.html_element_proto = html_element_proto;
    realm.svg_element_proto = svg_element_proto;
    realm.fragment_proto = fragment_proto;
    realm.tag_protos = tag_protos;
}

/// Welches Element welche Schnittstelle traegt.
///
/// Die Liste ist nicht vollstaendig und soll es nicht sein — sie deckt, was
/// Seiten abfragen. Was nicht daraufsteht, ist `HTMLElement`, und das ist
/// die richtige Antwort: ein unbekanntes Element IST eins, und `instanceof
/// HTMLElement` ist die Abfrage, die wirklich vorkommt. `HTMLUnknownElement`
/// waere formal genauer und praktisch nutzlos.
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

/// Der naechste/vorige ELEMENT-Geschwisterknoten. Wie `sibling`, nur laeuft
/// er weiter, bis ein Element kommt — Textknoten zwischen zwei `<li>` sind
/// der Normalfall, nicht die Ausnahme, und genau deshalb fragt Seitencode
/// nach `nextElementSibling` und nicht nach `nextSibling`.
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


// ── Ereigniszustellung ──────────────────────────────────────────────────────

/// Ein Ereignis an `target` zustellen und die Kette hinauf blasen.
///
/// `chain` kommt aus dem Layout: die `seq`-Kette unter dem Zeiger, vom
/// aeussersten zum innersten. Zugestellt wird UMGEKEHRT — vom Ziel nach
/// aussen, so wie ein Browser blaest. Die Einfangphase gibt es nicht; sie
/// braucht ein drittes Argument an `addEventListener`, das kaum eine Seite
/// benutzt, und ohne sie stimmt die Reihenfolge fuer alles Uebrige.
///
/// Liefert true, wenn ein Behandler `preventDefault` gerufen hat — dann
/// unterbleibt, was beak sonst getan haette (einem Link folgen).
/// Welche Ereignisse beak ueberhaupt zustellt.
///
/// Die Liste ist absichtlich kurz und deckungsgleich mit dem, was der Wirt
/// wirklich ausloest. Ein `onload` hier aufzunehmen wuerde jeder Seite
/// Treffer-Kaesten aufzwingen, die nie jemand befragt — Aufwand fuer ein
/// Ereignis, das nie kommt.
pub const DISPATCHED: &[&str] = &["click"];

/// Ist `k` ein Attribut-Behandler fuer eins davon?
fn is_handler_attr(k: &str) -> bool {
    k.len() > 2 && k.starts_with("on") && DISPATCHED.contains(&&k[2..])
}

/// Den Behandler aus `on<art>` uebersetzen, falls es einen gibt.
///
/// Ein Attribut ist Quelltext, keine Funktion — es wird erst beim Ausloesen
/// uebersetzt. Das kostet je Klick eine Uebersetzung von ein paar Dutzend
/// Zeichen und spart, die halbe Seite beim Laden zu uebersetzen: die meisten
/// dieser Behandler werden nie ausgeloest.
///
/// Ein Attribut, das sich nicht uebersetzen laesst, ist KEIN Fehler der
/// Seite: der Browser laesst es still fallen, sonst haette ein Tippfehler in
/// einem Attribut die ganze Zustellung angehalten.
fn inline_handler(i: &mut Interp, node: u32, kind: &str) -> C<Option<Value>> {
    let mut name = String::from("on");
    name.push_str(kind);
    let src = match &i.doc {
        Some(d) => match d.nodes[node as usize].attr(&name) { Some(v) => v.to_string(), None => return Ok(None) },
        None => return Ok(None),
    };
    if src.trim().is_empty() { return Ok(None); }
    // Der Koerper laeuft mit `event` als Namen und `this` am Element — genau
    // so ist der Attribut-Behandler definiert.
    let mut wrapped = String::from("(function(event){");
    wrapped.push_str(&src);
    wrapped.push_str("\n})");
    let prog = match super::parse(&wrapped, false) { Ok(p) => p, Err(_) => return Ok(None) };
    match i.run_program(&prog) {
        Ok(v) if i.is_callable(&v) => Ok(Some(v)),
        _ => Ok(None),
    }
}

/// Ein Ereignis, das beak selbst ausloest, ueber die Kette zustellen.
///
/// `chain` ist die Kette aus dem LAYOUT, aussen zuerst — nicht aus dem Baum.
/// Das ist keine Feinheit: der Klickpunkt kennt nur Kaesten, und wer die
/// Kette stattdessen aus dem Baum baut, prueft einen Weg, den beak nie geht
/// ([[feedback_the_test_path_must_be_the_real_path]]).
pub fn dispatch(i: &mut Interp, kind: &str, chain: &[u32]) -> C<bool> {
    if chain.is_empty() { return Ok(false); }
    let proto = i.realm.event_proto.clone();
    let ev = build_event(i, proto, kind, true);
    ev.borrow_mut().define(EV_BUBBLES, Prop { value: Some(Value::Bool(true)), get: None,
        set: None, writable: true, enumerable: false, configurable: true });
    let prevented = deliver(i, &ev, kind, chain)?;
    // Ein Klick ist eine AUFGABE — danach laeuft die Microtask-Schlange, wie
    // nach jeder anderen auch. Sonst bliebe ein `.then` aus dem Behandler bis
    // zum naechsten Zeitgeber liegen, und auf einer Seite ohne Zeitgeber
    // fuer immer.
    super::promise::run_jobs(i);
    Ok(prevented)
}

/// Der gemeinsame Kern: ein fertiges Ereignis ueber eine fertige Kette.
///
/// Zwei Wege enden hier — beaks eigener Klick und `el.dispatchEvent(…)` der
/// Seite. Ein zweiter Rumpf waere ein zweiter Satz Regeln, und der eine wuerde
/// gepflegt und der andere nicht.
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
        // Der Behandler aus dem Attribut oder aus der Eigenschaft zuerst: im
        // Quelltext steht er vor jedem `addEventListener`, das ein Skript
        // spaeter anmeldet, und die Reihenfolge der Anmeldung ist die
        // Reihenfolge des Aufrufs.
        //
        // ENTWEDER-ODER: `el.onclick = f` ersetzt im Browser den Behandler
        // aus dem Attribut, es ist derselbe Platz. Beide laufen zu lassen
        // hiesse, dass eine Zuweisung den alten nicht loswird.
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
        // 2 = AT_TARGET, 3 = BUBBLING_PHASE. Eine Fangphase gibt es nicht:
        // `addEventListener` nimmt das dritte Argument an und verwirft es.
        set(ev, EV_PHASE, Value::Num(if k + 1 == chain.len() { 2.0 } else { 3.0 }));
        for f in listeners {
            // Ein Behandler, der wirft, darf die naechsten nicht mitnehmen —
            // so macht es ein Browser auch.
            let r = i.call(&f, this_node.clone(), &[evv.clone()]);
            // **Ein Behandler, der wirft, muss es SAGEN.** Der Ausgang wurde
            // hier nur auf `false` geprueft und sonst weggeworfen: ein Fehler
            // im `onsubmit` einer Seite verschwand spurlos, und was danach
            // nicht passierte, sah aus wie ein fehlendes Merkmal.
            if let Err(e) = r {
                let msg = super::modules::describe(i, e);
                i.console_push(alloc::format!("Fehler im {kind}-Behandler: {msg}"));
                continue;
            }
            // `onclick="return false"` ist die alte Schreibweise fuer
            // `preventDefault` und steht auf mehr Seiten als die neue. Sie
            // gilt nur fuer den Attribut-Behandler; ein `addEventListener`
            // wertet den Rueckgabewert nicht aus.
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

    /// Ein Kasten, wie das Layout ihn aufzeichnet — kurz, weil eine Probe die
    /// zwoelf Felder sonst dreimal ausschreibt und nur fuenf davon meint.
    pub fn boxed(seq: u32, x: i32, y: i32, w: i32, h: i32, bx: i16, by: i16) -> ElemRect {
        ElemRect { seq, x, y, w, h, bx, by, px: 0, py: 0, positioned: false }
    }

    /// Derselbe Kasten mit Polsterung — nur die Proben der Beobachter
    /// brauchen sie.
    /// Und mit `position` — nur `offsetParent` fragt danach.
    pub fn placed(seq: u32, x: i32, y: i32, w: i32, h: i32) -> ElemRect {
        ElemRect { seq, x, y, w, h, bx: 0, by: 0, px: 0, py: 0, positioned: true }
    }

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
    /// `getComputedStyle` antwortet aus der KASKADE, nicht aus dem
    /// Inline-Stil.
    ///
    /// Der Unterschied ist der ganze Sinn: `.hide{display:none}` steht in
    /// einem Blatt, nicht am Element. Bis 0.64.0 gab die Funktion darauf
    /// „block" zurueck — eine Auskunft, auf die eine Seite ihren naechsten
    /// Schritt baut.
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

    /// **`new Image()` ist ein echtes `img`-Element, keine Attrappe.**
    ///
    /// Die Google-Ergebnisseite bricht ihren Zeitgeber sonst mit
    /// `Image is not defined` ab — ein Zaehlpixel besteht genau aus
    /// `new Image().src = …`. Ein Stummel haette den Fehler weggenommen und
    /// den Knoten schuldig geblieben; der Test prueft deshalb, dass das Ding
    /// im Baum ankommt und sich wie ein Element verhaelt.
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

    /// **Am Wurzelelement ist `clientWidth` die SICHTFLAECHE** (CSSOM View
    /// §4), nicht der Polsterkasten. Googles Startseite misst damit das
    /// Fenster und laeuft nur weiter, wenn beide Werte wahr sind — mit 0
    /// blieb ihr ganzer Messblock stehen und das Formular schickte
    /// `biw=&bih=`.
    ///
    /// Eigener Aufbau statt `run`: der Helfer setzt keine Sichtflaeche, und
    /// genau die ist hier der Gegenstand.
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

    /// Ein Element, das ein Skript erst erzeugt hat, kommt im Baum nicht vor.
    /// Dafuer gibt es keinen gerechneten Stil — und die leere Zeichenkette ist
    /// die ehrliche Antwort, keine erfundene Zahl.
    #[test]
    fn an_element_the_script_made_has_no_cascade_yet() {
        let out = run(
            "<html><body><p>x</p></body></html>",
            "var e = document.createElement('div');\
             console.log(JSON.stringify(getComputedStyle(e).display));",
        );
        assert_eq!(out, ["\"\""]);
    }

    /// Was das SKRIPT an den Inline-Stil schreibt, sieht der gerechnete Stil
    /// auch — obwohl der Kaskadenkontext ein Schnappschuss vom Skriptstart
    /// ist. Ohne diese Ueberlagerung antwortete `getComputedStyle` mit dem
    /// Stand von vorgestern, und ein Skript, das setzt und dann misst, bekam
    /// seinen eigenen Wert nicht zurueck.
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

    /// **Der Fall, wegen dem der Schnappschuss weg ist.** Ein Skript setzt
    /// eine KLASSE und misst dann — die Klasse entscheidet, welche Regeln
    /// ueberhaupt treffen, und aus einem Baum vom Skriptstart ist das nicht zu
    /// beantworten. Bis 0.74.0 kam die Antwort von vorher.
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

    /// Und ein Knoten, den das Skript erst EINHAENGT, bekommt seine Kaskade —
    /// samt allem, was ihm der Nachbar oder der Elternteil vererbt.
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

    /// Geometrie kommt aus dem LAYOUT, und der Wirt reicht sie ein. Ohne
    /// eingereichte Kaesten bleibt es bei Nullen — das ist die Antwort eines
    /// Browsers fuer ein Element ohne Kasten und die einzige ehrliche, solange
    /// es kein Layout gibt.
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
            // y ist um den Rollstand verschoben: 100 - 40.
            "10,60,200,50,210,110",
            // offsetTop geht gegen das DOKUMENT, also OHNE den Rollstand.
            "200,50,100",
            // Polsterkasten = Rahmenkasten ohne die Rahmensummen.
            "196,44",
            "1",
        ]);
    }

    /// Ein Kasten ueber mehrere Zeilen hat mehrere Fragmente. `getClientRects`
    /// nennt sie einzeln, `getBoundingClientRect` ihre VEREINIGUNG — nicht das
    /// erste, was man findet.
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


    /// `ResizeObserver` meldet den INHALTSkasten — Rahmen UND Polsterung ab.
    ///
    /// Das ist der Unterschied, der zaehlt: ein Diagramm baut sein Zeichenfeld
    /// aus `entry.contentRect.width`, und in einem gepolsterten Kasten waere
    /// jede andere Zahl zu gross. Vor dieser Runde fuehrte das Layout die
    /// Polsterung gar nicht mit.
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
        // Erst die Kaesten, dann das Skript: `observe` misst SOFORT.
        i.set_geometry(super::super::interp::Geometry {
            boxes: alloc::rc::Rc::new(alloc::vec![
                // 200x50 Rahmenkasten, 4 px Rahmen und 20 px Polsterung
                // waagerecht, 6 + 10 senkrecht.
                padded(seq, 10, 100, 200, 50, 4, 6, 20, 10),
            ]),
            scroll: (0, 0),
            content: (1024, 4000),
        });
        let _ = i.run_program(&prog);
        // Zugestellt wird am Kontrollpunkt, nicht im `observe`.
        i.run_timers();
        let prog = super::super::parse("console.log(log.join(' | '));", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["176x34 rand 200 inhalt 34"]);
    }

    /// Und er meldet nur, was sich GEAENDERT hat. Ein zweites Layout mit
    /// derselben Groesse ist kein Ereignis — sonst waere jeder Bildlauf ein
    /// Rueckrufgewitter.
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
        // Dasselbe noch dreimal — die Groesse steht.
        for _ in 0..3 { i.set_geometry(geom(200)); i.run_timers(); }
        // Jetzt bewegt sie sich.
        i.set_geometry(geom(300));
        i.run_timers();
        let prog = super::super::parse("console.log(n);", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(), ["2"]);
    }

    /// `IntersectionObserver`: das Verhaeltnis ist die geschnittene Flaeche
    /// geteilt durch die des Ziels, und gemeldet wird beim WECHSEL ueber eine
    /// Schwelle.
    #[test]
    fn intersection_observer_reports_when_a_threshold_is_crossed() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_viewport(1000.0, 1000.0);
        let seq = find_seq(&dom.root, "a").expect("das div");
        // Ein 100 hoher Kasten, dessen Oberkante bei y liegt. Bei y = 950
        // ragen 50 von 100 ins Sichtfeld: Verhaeltnis 0,5.
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
        // Ein Viertel herein: ueber 0, unter 0,5.
        i.set_geometry(at(975));
        i.run_timers();
        // Die Haelfte: die zweite Schwelle faellt.
        i.set_geometry(at(950));
        i.run_timers();
        // Wieder hinaus.
        i.set_geometry(at(2000));
        i.run_timers();
        let prog = super::super::parse("console.log(log.join(' | '));", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_console(),
                   ["false@0 | true@0.25 | true@0.5 | false@0"]);
    }

    /// `rootMargin` dehnt den Ausschnitt — genau das macht „lade das Bild,
    /// BEVOR es sichtbar wird" moeglich.
    #[test]
    fn a_root_margin_stretches_the_clip() {
        use super::beak_engine_layout_boxes::*;
        let html = "<html><body><div id=a>x</div></body></html>";
        let dom = crate::dom::parse(html);
        let mut i = super::super::interp::Interp::new();
        i.set_document(super::Doc::from_dom(&dom));
        i.set_viewport(1000.0, 1000.0);
        let seq = find_seq(&dom.root, "a").expect("das div");
        // 200 px UNTER dem Sichtfeld.
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


    /// Die Rollmasse kommen aus den KAESTEN, nicht aus einer 0.
    ///
    /// `scrollHeight` ist der Polsterkasten, vereinigt mit den Nachfahren —
    /// ein Kind, das aus seinem Elter laeuft, macht genau den Unterschied,
    /// wegen dem eine Seite ueberhaupt fragt.
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
                // 200x60 Rahmenkasten, 4 px Rahmen und 10 px Polsterung.
                padded(a, 0, 0, 200, 60, 4, 4, 10, 10),
                // Das Kind ist 400 hoch und laeuft unten heraus.
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
            // Polsterkasten: 200-4, 60-4.
            "196,56",
            // Rollflaeche: die Breite passt (das Kind endet bei 182, der
            // Polsterkasten ist 196 breit), die Hoehe nicht — das Kind endet
            // bei 402, die Polsterkante liegt bei 2.
            "196,400",
            // beak klemmt nichts ab: an einem gewoehnlichen Element ist
            // NICHTS weggerollt, und 0 ist hier wahr statt geraten.
            "0,0",
        ]);
    }

    /// Am Wurzelelement ist es die Rollflaeche des DOKUMENTS, und die sagt
    /// das Layout — nicht die Vereinigung der Kaesten.
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

    /// Rollen ist ein WUNSCH. Die Engine hat kein Fenster; sie merkt sich,
    /// was die Seite wollte, und der Wirt holt es ab.
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
        // Und `scrollIntoView` rechnet die Dokumentlage aus, nicht die
        // Fensterlage: der Kasten steht bei 1200, gerollt ist nichts.
        let prog = super::super::parse(
            "document.getElementById('p').scrollIntoView();", false).expect("parst");
        let _ = i.run_program(&prog);
        assert_eq!(i.take_scroll(), Some((None, Some(1200.0))));
        // Zweimal abholen gibt beim zweiten Mal nichts.
        assert_eq!(i.take_scroll(), None);
    }

    /// `offsetParent` ist der naechste POSITIONIERTE Vorfahr — und die Ecke
    /// dafuer faehrt im Layoutkasten mit, statt fuer jeden Vorfahren die
    /// Kaskade neu aufzuloesen.
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
                placed(a, 0, 0, 300, 300),          // positioniert
                boxed(m, 0, 0, 300, 200, 0, 0),     // nicht
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

    /// Der Kleinkram, in EINEM Lauf: jede dieser Zeilen war vorher ein
    /// `TypeError` mitten in fremdem Code.
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
            // Anmeldedaten faehrt beak nicht — und sagt das, statt zu
            // schweigen oder sie in die Adresszeile zu schreiben.
            "[][]http://example.com/p",
        ]);
    }

    /// Der Inline-Stil bleibt eine LEBENDE Sicht — `el.style` ist etwas
    /// anderes als `getComputedStyle(el)`, und beide gehen durch dieselben
    /// Zugriffsfunktionen.
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

/// Das interne Feld eines `TextDecoder`: verwirft eine ungueltige Folge
/// still, oder wirft? NUL-praefigiert, also fuer jedes Skript unsichtbar.
const TD_FATAL: &str = "\0!tdfatal";

/// `TextEncoder` und `TextDecoder`.
///
/// **Nur UTF-8, und das ist keine Luecke.** Die Spezifikation laesst dem
/// Encoder gar keine andere Wahl (`new TextEncoder("latin1")` ist trotzdem
/// UTF-8), und der Decoder nimmt zwar Beschriftungen entgegen, aber jede
/// Seite, die eine andere als UTF-8 braucht, braucht auch eine Tabelle, die
/// hier nicht liegt. Eine fremde Beschriftung wird also angenommen und wie
/// UTF-8 behandelt, statt zu werfen — der Fritzbox-Anmeldecode ruft
/// `new TextEncoder("utf-8")`, und ein Wurf dort waere ein Fehler ueber
/// nichts.
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
    // `encodeInto` schreibt in eine bestehende Sicht und meldet, wie weit es
    // gekommen ist. Abgeschnitten wird an einer ZEICHENgrenze — eine halbe
    // Folge in den Puffer zu legen waere kaputtes UTF-8.
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
                // Ohne `fatal` ersetzt die Spezifikation jede ungueltige
                // Folge durch U+FFFD, statt zu werfen.
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

/// Eine frische `Uint8Array` mit diesen Bytes.
fn bytes_to_u8(i: &mut Interp, b: &[u8]) -> Value {
    let v = i.new_typed(ElemKind::U8, b.len());
    write_view(&v, b);
    v
}

/// Die Bytes hinter einer Sicht ODER einem Puffer. Alles andere ist leer —
/// `decode` bekommt in echtem Code nie etwas anderes.
fn read_view(v: &Value) -> Vec<u8> {
    let Value::Obj(o) = v else { return Vec::new() };
    // Erst die Angaben herausholen, dann die Ausleihe fallen lassen: eine
    // Sicht auf SICH SELBST gibt es nicht, aber `slice_of` leiht den Puffer
    // erneut, und bei `ObjKind::Buffer` waere das dasselbe Objekt.
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

/// Wieviele BYTES in die Sicht passen.
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

/// UTF-8 mit U+FFFD fuer jede ungueltige Folge. `String::from_utf8_lossy`
/// gibt es in `alloc` — aber nur mit `Cow`, und die Grenze ist hier
/// uninteressant.
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

/// `customElements` — die Registratur der eigenen Elemente.
///
/// **Ohne Schattenbaum.** `attachShadow` fehlt weiter; gemessen an der
/// Fritzbox-Oberflaeche benutzen vier von vierzehn Komponenten einen, und
/// keine davon steht auf der Anmeldeseite. Die Trennung ist bewusst: ein
/// halber Schattenbaum waere schlimmer als keiner, weil er das Layout
/// betrifft und nicht nur die Bindung.
///
/// **Was es kann:** anmelden, nachschlagen, und — der eigentliche Punkt —
/// `class X extends HTMLElement` KONSTRUIERBAR machen. `new X()` legt einen
/// echten Knoten mit der angemeldeten Marke an; welche Marke, sagt der
/// Prototyp des gerade gebauten Objekts.
fn install_custom_elements(realm: &mut Realm, html_element_proto: &Gc) {
    let fp = realm.function_proto.clone();

    // `HTMLElement` ist ab hier ein echter Konstruktor. Ohne ihn wirft
    // `super()` in jeder Komponente „Illegal constructor" — und das ist die
    // erste Zeile, die eine Seite mit Web Components ausfuehrt.
    let he = native(Some(fp.clone()), |i, this, _| {
        if !i.native_new { return i.type_err("Illegal constructor"); }
        let Some(tag) = custom_tag_of(i, &this) else {
            return i.type_err("HTMLElement constructor: the class is not a registered custom element");
        };
        let Some(d) = &mut i.doc else { return i.type_err("no document") };
        let id = d.create(ELEMENT_NODE, &tag);
        Ok(wrap(i, id))
    }, "HTMLElement", 0, true);
    he.borrow_mut().define("prototype", Prop::frozen(Value::Obj(html_element_proto.clone())));
    html_element_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(he.clone())));
    realm.global.borrow_mut().define("HTMLElement", Prop::builtin(Value::Obj(he)));

    let ce = new_obj(Some(realm.object_proto.clone()));
    meth(&ce, "define", |i, _, a| {
        let name = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let ctor = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&ctor) { return i.type_err("customElements.define: not a constructor"); }
        // Eine Marke ohne Bindestrich ist keine eigene — die Spezifikation
        // wirft dort, und eine Seite, die es doch versucht, will es wissen.
        if !name.contains('-') {
            return i.type_err(&alloc::format!("'{name}' is not a valid custom element name"));
        }
        if i.custom.iter().any(|(t, _)| **t == *name) {
            return i.type_err(&alloc::format!("'{name}' has already been defined"));
        }
        // **`observedAttributes` wird HIER gelesen**, nicht erst beim ersten
        // Attributwechsel. Die Spezifikation sagt es so, und Bibliotheken
        // haengen ihre ganze Einrichtung an diesen Zugriff: der `static get`
        // der Fritzbox-Komponenten ruft `finalize()`, und ohne den steht
        // spaeter `_wcProperties` auf `undefined`.
        let obs = i.get(&ctor, "observedAttributes")?;
        if !matches!(obs, Value::Undefined | Value::Null) {
            // Nur LESEN. Die Liste selbst braucht beak noch nicht — sie wird
            // gebraucht, wenn `attributeChangedCallback` kommt.
            let _ = i.iterate(&obs);
        }
        i.custom.push((name, ctor));
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&ce, "get", |i, _, a| {
        let name = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(i.custom.iter().find(|(t, _)| **t == *name).map(|(_, c)| c.clone())
            .unwrap_or(Value::Undefined))
    }, 1, &fp);
    meth(&ce, "getName", |i, _, a| {
        let c = a.first().cloned().unwrap_or(Value::Undefined);
        Ok(i.custom.iter().find(|(_, x)| x.strict_eq(&c))
            .map(|(t, _)| Value::Str(t.clone())).unwrap_or(Value::Null))
    }, 1, &fp);
    // `whenDefined` wird nur abgewartet. Da alle Anmeldungen beim Laden
    // passieren, ist die Antwort immer schon da.
    meth(&ce, "whenDefined", |i, _, a| {
        let name = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let v = i.custom.iter().find(|(t, _)| **t == *name).map(|(_, c)| c.clone())
            .unwrap_or(Value::Undefined);
        let p = super::promise::new_promise(i);
        super::promise::resolve_promise(i, &p, v);
        Ok(Value::Obj(p))
    }, 1, &fp);
    // `upgrade` tut nichts: beak baut den Baum aus dem HTML, bevor ein
    // Skript laeuft, und hebt vorhandene Knoten nicht nachtraeglich in eine
    // Klasse. Es ist da, damit ein Aufruf nicht wirft.
    meth(&ce, "upgrade", |_, _, _| Ok(Value::Undefined), 1, &fp);
    ce.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("CustomElementRegistry")));
    realm.global.borrow_mut().define("customElements", Prop::builtin(Value::Obj(ce)));
}

/// Welche angemeldete Marke gehoert zu diesem Objekt?
///
/// Ueber die PROTOTYPKETTE, von innen nach aussen — das gibt automatisch die
/// abgeleitetste Klasse. `new.target` gaebe dieselbe Antwort, aber beak
/// reicht es nicht durch, und die Kette weiss es ohnehin: `construct` hat den
/// Prototyp der gebauten Klasse schon gesetzt, bevor `super()` lief.
fn custom_tag_of(i: &Interp, this: &Value) -> Option<Rc<str>> {
    let Value::Obj(o) = this else { return None };
    let mut cur = o.borrow().proto.clone();
    while let Some(p) = cur {
        for (tag, ctor) in &i.custom {
            let Value::Obj(c) = ctor else { continue };
            let cp = c.borrow().get_own("prototype").and_then(|x| x.value.clone());
            if matches!(cp, Some(Value::Obj(cp)) if Rc::ptr_eq(&cp, &p)) {
                return Some(tag.clone());
            }
        }
        let n = p.borrow().proto.clone();
        cur = n;
    }
    None
}

/// Den DOM-Knoten von einem Huellenobjekt auf ein anderes umhaengen.
///
/// `super()` kopiert die Felder des eingebauten Ergebnisses in das `this` der
/// abgeleiteten Klasse. Der Knoten zeigt danach aber noch auf die
/// weggeworfene Huelle — und `wrap` gibt jedem, der ihn spaeter aus dem Baum
/// holt, genau die. Diese Zeile ist der Unterschied zwischen „die Komponente
/// IST das Element" und „es gibt sie zweimal".
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

/// Die `<option>`-Knoten eines `<select>`, in Dokumentreihenfolge und durch
/// `<optgroup>` hindurch — genau wie `forms.rs::collect_options`.
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

/// Der Wert einer Option: das Attribut, sonst ihr Text. Dieselbe Regel wie im
/// Layout — sonst zeigt die Auswahl etwas anderes an, als das Skript liest.
fn option_value(i: &Interp, id: u32) -> String {
    let Some(d) = &i.doc else { return String::new() };
    match d.nodes[id as usize].attr("value") {
        Some(v) => v.to_string(),
        None => d.text_of(id).trim().to_string(),
    }
}

/// Zu welchem `<select>` gehoert diese Option?
fn owning_select(i: &Interp, id: u32) -> Option<u32> {
    let d = i.doc.as_ref()?;
    let mut cur = d.nodes[id as usize].parent;
    while let Some(p) = cur {
        if &*d.nodes[p as usize].tag == "select" { return Some(p) }
        cur = d.nodes[p as usize].parent;
    }
    None
}

/// Welche Option ist ausgewaehlt?
///
/// Steht nirgends `selected`, ist es bei einer einfachen Auswahl die ERSTE —
/// so zeigt ein Browser sie an, und ein Skript, das gleich danach `value`
/// liest, bekaeme sonst die leere Zeichenkette.
fn selected_index(i: &Interp, sel: u32) -> f64 {
    let opts = select_options(i, sel);
    let Some(d) = &i.doc else { return -1.0 };
    for (n, o) in opts.iter().enumerate() {
        if d.nodes[*o as usize].attr("selected").is_some() { return n as f64 }
    }
    let multiple = d.nodes[sel as usize].attr("multiple").is_some();
    if !multiple && !opts.is_empty() { 0.0 } else { -1.0 }
}

/// Die n-te Option auswaehlen und alle anderen abwaehlen. `-1` waehlt nichts.
fn select_index(i: &mut Interp, sel: u32, n: i64) {
    let opts = select_options(i, sel);
    let Some(d) = &mut i.doc else { return };
    d.touch();
    for (k, o) in opts.iter().enumerate() {
        if k as i64 == n { d.set_attr_at(*o, "selected", ""); }
        else { d.remove_attr_at(*o, "selected"); }
    }
}

/// Den Inhalt eines Knotens durch EINEN Textknoten ersetzen — dieselbe
/// Regel wie `textContent`, nur als Funktion, weil `option.text` sie auch
/// braucht.
fn set_text_of(i: &mut Interp, id: u32, s: &str) {
    let Some(d) = &mut i.doc else { return };
    d.touch();
    let old: Vec<u32> = d.nodes[id as usize].children.clone();
    for c in old { d.nodes[c as usize].parent = None; }
    d.nodes[id as usize].children.clear();
    if !s.is_empty() {
        let tid = d.create(TEXT_NODE, "#text");
        d.nodes[tid as usize].text = s.into();
        d.append(id, tid);
    }
}

/// Vermerk auf der Huelle: `connectedCallback` ist gelaufen. NUL-praefigiert,
/// also fuer jedes Skript unsichtbar.
const CE_CONNECTED: &str = "\0!ceconn";

/// Haengt dieser Knoten wirklich am Dokument?
fn is_connected(d: &Doc, mut id: u32) -> bool {
    loop {
        if id == d.doc { return true }
        match d.nodes[id as usize].parent { Some(p) => id = p, None => return false }
    }
}

/// Die eigenen Elemente eines Teilbaums, von aussen nach innen.
fn collect_custom(d: &Doc, id: u32, out: &mut Vec<u32>) {
    let n = &d.nodes[id as usize];
    // Eine Marke OHNE Bindestrich kann kein eigenes Element sein — die
    // Spezifikation verlangt ihn, und die Pruefung kostet ein Byte.
    if n.kind == ELEMENT_NODE && n.tag.contains('-') { out.push(id); }
    for c in n.children.clone() { collect_custom(d, c, out); }
}

/// `connectedCallback` fuer alles, was gerade ins Dokument gekommen ist.
///
/// **Einmal je Element**, gemerkt auf der Huelle: die Fritzbox-Komponenten
/// bauen darin ihren Inhalt, und ein zweiter Lauf wuerde ihn verdoppeln.
/// Ein Wurf im Rueckruf beendet NICHT das Einhaengen — so macht es ein
/// Browser auch —, landet aber sichtbar auf der Konsole statt still zu
/// verschwinden.
fn fire_connected(i: &mut Interp, id: u32) -> C<Value> {
    settle_stylesheet(i, id);
    if i.custom.is_empty() { return Ok(Value::Undefined) }
    if !i.doc.as_ref().is_some_and(|d| is_connected(d, id)) { return Ok(Value::Undefined) }
    let mut list = Vec::new();
    if let Some(d) = &i.doc { collect_custom(d, id, &mut list); }
    for n in list {
        let v = wrap(i, n);
        let Value::Obj(o) = &v else { continue };
        if o.borrow().get_own(CE_CONNECTED).is_some() { continue }
        o.borrow_mut().define(CE_CONNECTED, Prop::frozen(Value::Bool(true)));
        let f = i.get(&v, "connectedCallback")?;
        if !i.is_callable(&f) { continue }
        if let Err(e) = i.call(&f, v.clone(), &[]) {
            let msg = super::modules::describe(i, e);
            let tag = i.doc.as_ref().map(|d| d.nodes[n as usize].tag.to_string()).unwrap_or_default();
            i.console_push(alloc::format!("connectedCallback <{tag}>: {msg}"));
        }
    }
    Ok(Value::Undefined)
}

/// Ein `<link rel="stylesheet">`, das ein SKRIPT einhaengt, wird zum HOLEN
/// angemeldet.
///
/// Die Engine holt nichts — sie legt die Adresse in `pending_sheets`, der
/// Wirt laedt sie und meldet mit `sheet_done` zurueck. Erst dann faellt
/// `load` oder `error` am `<link>`.
///
/// **Warum das eine eigene Runde wert ist.** Eine Seite, die ihre
/// Stilblaetter per Skript nachlaedt, wartet in aller Regel auf deren `load`,
/// bevor sie weiterbaut. Ohne Antwort steht sie fuer immer; mit einer
/// erfundenen `load`-Meldung baut sie weiter und sieht falsch aus, ohne dass
/// es jemand sagt. Beides ist schlechter als die Wahrheit, und die Wahrheit
/// heisst: holen.
fn settle_stylesheet(i: &mut Interp, id: u32) {
    let is_sheet = i.doc.as_ref().is_some_and(|d| {
        let n = &d.nodes[id as usize];
        &*n.tag == "link"
            && n.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet"))
            && n.attr("href").is_some_and(|h| !h.trim().is_empty())
            && is_connected(d, id)
    });
    if !is_sheet { return }
    // Zweimal anmelden waere zweimal holen — und zweimal `load`.
    if i.pending_sheets.iter().any(|(n, _)| *n == id) { return }
    let href = i.doc.as_ref().and_then(|d| d.nodes[id as usize].attr("href").cloned())
        .unwrap_or_else(|| Rc::from(""));
    i.pending_sheets.push((id, href.to_string()));
}

/// Der Wirt meldet, wie es einem angeforderten Blatt ergangen ist.
pub fn sheet_done(i: &mut Interp, id: u32, ok: bool) {
    let _ = dispatch(i, if ok { "load" } else { "error" }, &[id]);
}

/// Eine unbehandelte Ablehnung ans Fenster melden. Liefert true, wenn ein
/// Behandler `preventDefault` gerufen hat — dann unterbleibt die Konsolenzeile.
pub fn dispatch_rejection(i: &mut Interp, reason: Value, promise: Value) -> C<bool> {
    let Some(target) = i.doc.as_ref().map(|d| d.doc) else { return Ok(false) };
    let proto = i.realm.prej_proto.clone();
    let ev = build_event(i, proto, "unhandledrejection", true);
    ev.borrow_mut().define(EV_REASON, Prop::data(reason));
    ev.borrow_mut().define(EV_PROMISE, Prop::data(promise));
    deliver(i, &ev, "unhandledrejection", &[target])
}

/// `focus`/`blur` zustellen. Sie BLUBBERN NICHT — die Kette ist das Element
/// allein; `focusin`/`focusout` waeren die blubbernden Zwillinge.
fn deliver_focus(i: &mut Interp, id: u32, kind: &str) -> C<()> {
    dispatch(i, kind, &[id])?;
    Ok(())
}

/// Der WERT eines Steuerelements: der schmutzige, sonst der Vorgabewert.
///
/// Bei `<textarea>` ist der Vorgabewert der TEXTinhalt, bei `<input>` das
/// `value`-Attribut. Beides steht so in der Spezifikation, und beides ist der
/// Grund, warum es hier eine Funktion gibt statt zweier Makroaufrufe.
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

/// Die Steuerelemente eines `<form>`, in Dokumentreihenfolge.
///
/// Ueber den BAUM, nicht ueber `form=`: das Attribut, mit dem ein Element
/// ausserhalb seines Formulars stehen kann, liest beak nirgends, und eine
/// halbe Zuordnung waere schlimmer als eine ehrliche.
fn form_controls(i: &Interp, form: u32) -> Vec<u32> {
    fn walk(d: &Doc, id: u32, out: &mut Vec<u32>) {
        for c in d.nodes[id as usize].children.clone() {
            if d.nodes[c as usize].kind != ELEMENT_NODE { continue }
            let tag = &*d.nodes[c as usize].tag;
            if matches!(tag, "input" | "select" | "textarea" | "button" | "fieldset" | "output") {
                out.push(c);
            }
            // Ein verschachteltes `<form>` ist ungueltiges HTML; seine
            // Elemente gehoeren ihm, nicht uns.
            if tag != "form" { walk(d, c, out); }
        }
    }
    let mut out = Vec::new();
    if let Some(d) = &i.doc { walk(d, form, &mut out); }
    out
}

/// Alle Elemente einer Marke im Teilbaum, in Dokumentreihenfolge.
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

/// Ein Ereignis an das Element mit dieser `seq` zustellen — mit der Kette bis
/// zur Wurzel, also BLUBBERND.
///
/// Der Weg, auf dem der Wirt der Seite etwas meldet, das er selbst ausloest:
/// ein Formular, das abgeschickt wird, ein Element, das den Fokus bekommt.
/// Liefert true, wenn ein Behandler `preventDefault` gerufen hat (oder
/// `false` zurueckgab).
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

// ── Die Bruecke zwischen den Eingaben des Benutzers und dem Baum ─────────
//
// Zwei Speicher, EINE Regel. Die Eingaben leben im Wirt (`FormState`, nach
// `seq`), weil eine Seite ohne Skripte gar keinen Baum der Maschine hat; der
// schmutzige Wert lebt am Knoten, weil `el.value` ihn dort erwartet. Die
// beiden muessen vor und nach jedem Lauf von Seitencode abgeglichen werden —
// und dafuer gibt es genau diese zwei Funktionen, damit nicht jeder Rufer
// seine eigene Regel bekommt.

/// Die Eingaben des Benutzers in den Baum schreiben — VOR jedem Lauf von
/// Seitencode. Uebertragen wird nur, was wirklich bearbeitet wurde: sonst
/// traegt hinterher jedes Feld einen schmutzigen Wert und `form.reset()`
/// haette nichts mehr zurueckzustellen.
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

/// Und zurueck: was Seitencode gesetzt hat, gilt fuer Anzeige und Absenden.
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
