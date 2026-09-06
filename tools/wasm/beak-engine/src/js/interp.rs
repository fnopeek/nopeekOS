//! Die Auswertung: ein Baumlaeufer.
//!
//! **Warum ein Baumlaeufer und kein Bytecode.** Das Gedaechtnis notiert zu
//! Recht, dass die Form der Verteilerschleife eine Entwurfsentscheidung ist
//! und dass wasms `return_call` dort der moderne Hebel waere. Der Hebel bleibt
//! richtig — er wird nur nicht als erstes gezogen: heute gibt es keine Zahl,
//! gegen die er sich messen liesse, und was zuerst gebraucht wird, ist
//! Richtigkeit. Der test262-Lauf ist danach das Netz, mit dem eine Umstellung
//! auf Bytecode ueberhaupt erst verantwortbar ist.
//!
//! Was hier bewusst NICHT steht: Generatoren, async/await, Proxy, Symbole,
//! BigInt. Jedes davon faellt im Lauf als eigene Zeile auf und ist damit
//! gezaehlt statt vergessen.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::{HashMap, HashSet};

use super::ast::*;
use super::value::*;
pub use super::value::Value;

/// Ein Abbruch: alles, was nicht „der naechste Ausdruck" ist.
pub enum Abrupt {
    Throw(Value),
    Return(Value),
    Break(Option<String>),
    Continue(Option<String>),
}
pub type C<T> = Result<T, Abrupt>;

pub struct Binding {
    pub value: Value,
    pub mutable: bool,
    /// `let`/`const` vor ihrer Deklaration: der Zugriff wirft. Ohne das ist
    /// die zeitliche Totzone unsichtbar und `let` verhaelt sich wie `var`.
    pub initialized: bool,
}

pub struct Env {
    pub vars: HashMap<Rc<str>, Binding>,
    pub parent: Option<Rc<RefCell<Env>>>,
    /// Nur Funktionsumgebungen tragen `this`; ein Block erbt es. Genau daran
    /// haengt, dass ein Pfeil das `this` seiner Umgebung sieht.
    pub this_val: Option<Value>,
    /// Ist das die Umgebung einer Funktion (Ziel fuer `var`-Hochziehen)?
    pub is_func_scope: bool,
    /// Das „Heimatobjekt" der Methode, in der wir stehen — bei einer Klasse
    /// ihr `prototype`. `super.f` sucht auf DESSEN Prototyp, nicht auf dem
    /// von `this`: sonst faende eine Methode, die `super.f()` ruft, sich
    /// selbst wieder und liefe endlos. Ein Pfeil setzt es nicht und erbt es
    /// dadurch ueber die Kette, genau wie `this`.
    pub home: Option<Gc>,
    /// Streng? Die Strenge steht am CODE (siehe [[ast::Func::strict]]), aber
    /// gelesen wird sie zur Laufzeit — an jeder Zuweisung, jedem `delete`,
    /// jedem `this`. Deshalb faellt sie beim Anlegen der Umgebung hier
    /// hinein und wird VERERBT: ein Block in einer strengen Funktion ist
    /// streng, ohne dass jemand die Kette hochlaufen muss.
    pub strict: bool,
    /// Namen, die aus einem ANDEREN Modul kommen (`import`).
    ///
    /// **Ein Verweis, keine Kopie.** Ein Modulgraph mit Zyklen — und der der
    /// Fritzbox-Oberflaeche hat welche (`main` <-> `oldpage` <-> `html2`) —
    /// braucht LEBENDE Bindungen: wer im Kreis frueher laeuft, sieht den
    /// Wert, den der andere spaeter hineinschreibt. Eine Kopie beim Verbinden
    /// saehe dort `undefined`, und zwar still.
    ///
    /// Nur Modulumgebungen tragen die Tabelle; jede andere ein `None`, und
    /// das ist eine Nullpruefung im Kettenlauf.
    pub imports: Option<Box<HashMap<Rc<str>, (Rc<RefCell<Env>>, Rc<str>)>>>,
}

impl Env {
    /// Erbt die Strenge vom Elter. Ein Funktionsaufruf ueberschreibt sie
    /// gleich danach mit der seines eigenen Rumpfes — nur DORT darf sie sich
    /// aendern, und nur nach oben.
    pub fn new(parent: Option<Rc<RefCell<Env>>>, func_scope: bool) -> Rc<RefCell<Env>> {
        let strict = parent.as_ref().is_some_and(|p| p.borrow().strict);
        Rc::new(RefCell::new(Env {
            vars: HashMap::new(), parent, this_val: None, is_func_scope: func_scope, home: None,
            strict, imports: None,
        }))
    }
}

/// Ist der Code, der gerade laeuft, streng? Ein Feld, kein Kettenlauf — die
/// Vererbung ist beim Anlegen passiert.
pub fn env_strict(env: &Rc<RefCell<Env>>) -> bool { env.borrow().strict }

pub fn env_lookup(env: &Rc<RefCell<Env>>, name: &str) -> Option<Rc<RefCell<Env>>> {
    let mut cur = env.clone();
    loop {
        {
            let b = cur.borrow();
            if b.vars.contains_key(name) { drop(b); return Some(cur); }
            if b.imports.as_ref().is_some_and(|m| m.contains_key(name)) { drop(b); return Some(cur); }
        }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return None }
    }
}

/// Einem `import` folgen, bis eine echte Bindung dasteht.
///
/// Eine Kette ist moeglich (`export { x } from …` reicht durch), ein KREIS
/// auch — ein Modul, das seinen eigenen Namen wieder einfuehrt. Der Deckel
/// ist deshalb kein Vorsichtsmass, sondern die Abbruchbedingung.
pub fn env_deref(env: &Rc<RefCell<Env>>, name: &str) -> Option<(Rc<RefCell<Env>>, Rc<str>)> {
    let mut e = env.clone();
    let mut n: Rc<str> = Rc::from(name);
    for _ in 0..64 {
        let next = {
            let b = e.borrow();
            if b.vars.contains_key(&*n) { None }
            else { b.imports.as_ref().and_then(|m| m.get(&n).cloned()) }
        };
        match next {
            None => return Some((e, n)),
            Some((e2, n2)) => { e = e2; n = n2; }
        }
    }
    None
}

/// Das Heimatobjekt der naechsten umschliessenden Methode.
pub fn env_home(env: &Rc<RefCell<Env>>) -> Option<Gc> {
    let mut cur = env.clone();
    loop {
        if let Some(h) = cur.borrow().home.clone() { return Some(h); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return None }
    }
}

/// `this`, PLUS die Frage, ob der Modus daran etwas aendern wuerde.
///
/// Beide Maschinen rufen sie, damit die Sonde nicht in zwei Fassungen
/// auseinanderlaeuft. `undefined`/`null` waere im lockeren Modus `globalThis`,
/// ein Primitiv waere dort eingepackt — beides sieht das Programm.
pub fn this_observed(i: &mut Interp, env: &Rc<RefCell<Env>>) -> Value {
    let v = env_this(env);
    match &v {
        Value::Undefined | Value::Null => strict_site!(i, 8),
        Value::Obj(_) => {}
        _ => strict_site!(i, 9),
    }
    v
}

/// `this` NEU binden — in genau der Umgebung, die es traegt.
///
/// Nur `super()` braucht das: bis dahin ist `this` in einer abgeleiteten
/// Klasse noch nicht endgueltig, und ein Elternkonstruktor, der ein Objekt
/// zurueckgibt, entscheidet es.
pub fn set_env_this(env: &Rc<RefCell<Env>>, v: Value) {
    let mut cur = env.clone();
    loop {
        if cur.borrow().this_val.is_some() { cur.borrow_mut().this_val = Some(v); return; }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return }
    }
}

pub fn env_this(env: &Rc<RefCell<Env>>) -> Value {
    let mut cur = env.clone();
    loop {
        if let Some(t) = &cur.borrow().this_val { return t.clone(); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return Value::Undefined }
    }
}

/// Die eingebauten Objekte einer Ausfuehrungseinheit.
pub struct Realm {
    pub global: Gc,
    pub global_env: Rc<RefCell<Env>>,
    pub object_proto: Gc,
    pub function_proto: Gc,
    pub array_proto: Gc,
    pub string_proto: Gc,
    pub number_proto: Gc,
    pub boolean_proto: Gc,
    pub error_proto: Gc,
    /// Name -> Prototyp der Fehlerarten, fuer `throw_type` & Co.
    pub error_ctors: HashMap<&'static str, Gc>,
    pub node_proto: Gc,
    pub element_proto: Gc,
    pub text_proto: Gc,
    pub document_proto: Gc,
    /// `Event.prototype`. Liegt im Realm, weil die Zustellung Ereignisse
    /// BAUT und die eingebauten Funktionen Zeiger sind, keine Abschluesse —
    /// sie koennen den Prototyp nicht einfangen.
    pub event_proto: Gc,
    pub token_list_proto: Gc,
    pub comment_proto: Gc,
    pub style_proto: Gc,
    pub regexp_proto: Gc,
    pub symbol_proto: Gc,
    /// `%IteratorPrototype%` — der gemeinsame Vorfahr aller eingebauten
    /// Iteratoren. Er traegt `[Symbol.iterator]() { return this }`, und genau
    /// daran haengt, dass ein Iterator selbst wieder iterierbar ist.
    pub iterator_proto: Gc,
    /// `%GeneratorPrototype%` (`next`/`return`/`throw`) und
    /// `%GeneratorFunction.prototype%`. Sie liegen im Realm, weil eingebaute
    /// Funktionen Zeiger sind und keine Abschluesse — sie koennen den
    /// Prototyp nicht einfangen.
    pub generator_proto: Gc,
    pub generator_func_proto: Gc,
    pub array_iter_proto: Gc,
    pub string_iter_proto: Gc,
    pub promise_proto: Gc,
    pub date_proto: Gc,
    pub bigint_proto: Gc,
    pub iter_helper_proto: Gc,
    pub iter_wrap_proto: Gc,
    /// Die eingebaute `eval`. Gemerkt, weil ein Aufruf nur dann ein DIREKTER
    /// ist, wenn er GENAU sie trifft.
    pub eval_fn: Option<Gc>,
    /// Die Schnittstellen-Prototypen der DOM-Bindung. `tag_protos` bildet den
    /// Elementnamen auf seine Schnittstelle ab; was nicht darinsteht, ist
    /// `HTMLElement`.
    pub html_element_proto: Gc,
    pub svg_element_proto: Gc,
    pub fragment_proto: Gc,
    pub tag_protos: HashMap<&'static str, Gc>,
    pub url_proto: Gc,
    pub url_params_proto: Gc,
    pub prej_proto: Gc,
    pub text_encoder_proto: Gc,
    pub text_decoder_proto: Gc,
    /// Die Prototypen der neun Sichten, nach ihrem Namen — `new_typed` haengt
    /// eine frische Sicht daran, und eingebaute Funktionen sind Zeiger, die
    /// nichts einfangen koennen.
    pub ta_protos: HashMap<&'static str, Gc>,
    pub typed_proto: Gc,
    pub buffer_proto: Gc,
    pub dataview_proto: Gc,
}

impl Realm {
    /// Alles, was der Realm selbst festhaelt. Eine Liste und keine
    /// Aufzaehlung von Hand an jeder Stelle: wer ein Feld hinzufuegt, sieht
    /// hier, dass es auch abgebaut werden muss.
    fn roots(&self) -> Vec<Gc> {
        alloc::vec![
            self.global.clone(), self.object_proto.clone(), self.function_proto.clone(),
            self.array_proto.clone(), self.string_proto.clone(), self.number_proto.clone(),
            self.boolean_proto.clone(), self.error_proto.clone(), self.node_proto.clone(),
            self.element_proto.clone(), self.text_proto.clone(), self.document_proto.clone(),
            self.event_proto.clone(), self.token_list_proto.clone(), self.style_proto.clone(),
            self.comment_proto.clone(), self.regexp_proto.clone(), self.symbol_proto.clone(),
            self.iterator_proto.clone(), self.generator_proto.clone(),
            self.generator_func_proto.clone(), self.array_iter_proto.clone(),
            self.string_iter_proto.clone(), self.promise_proto.clone(),
            self.typed_proto.clone(), self.buffer_proto.clone(),
            self.dataview_proto.clone(),
            self.html_element_proto.clone(), self.svg_element_proto.clone(),
            self.fragment_proto.clone(), self.url_proto.clone(), self.url_params_proto.clone(),
        ]
    }
}

impl Drop for Interp {
    fn drop(&mut self) { self.teardown(); }
}

/// Der Kaskadenkontext, den der Wirt einreicht.
/// Die Kaesten des LETZTEN Layouts — was `getBoundingClientRect` und die
/// `offset*`/`client*`-Felder beantworten.
///
/// **Warum das der Wirt einreicht und die Maschine es nicht selbst rechnet:**
/// Geometrie entsteht im Layout, und das Layout ist beaks Sache. Dieselbe
/// Bauart wie `StyleCtx` und `set_media`.
///
/// **Und warum es die Kaesten von VORHIN sind:** ein Skript, das den Baum
/// aendert und sofort misst, bekommt hier den Stand vor seiner Aenderung. Ein
/// Browser legt an dieser Stelle synchron neu aus — auf Wikipedia gemessene
/// 70 ms je Abfrage. Das waere hier machbar und ist bewusst nicht gebaut:
/// erst soll jemand eine Seite zeigen, der es weh tut. Bis dahin ist die
/// letzte echte Geometrie ungleich besser als die Null, die vorher dastand.
pub struct Geometry {
    /// Ein Eintrag je FRAGMENT — ein Inline-Kasten ueber drei Zeilen hat drei,
    /// und das ist richtig: `getClientRects` nennt sie einzeln,
    /// `getBoundingClientRect` ihre Vereinigung.
    pub boxes: alloc::rc::Rc<alloc::vec::Vec<crate::layout::ElemRect>>,
    /// Der Rollstand des Fensters. Die Kaesten stehen in Dokumentkoordinaten,
    /// `getBoundingClientRect` antwortet in Fensterkoordinaten.
    pub scroll: (i32, i32),
}

pub struct StyleCtx {
    pub sheet: alloc::rc::Rc<crate::css::Stylesheet>,
    pub theme: crate::layout::Theme,
    pub viewport_w: f32,
}

/// Die Stellen, an denen der strenge Modus abweicht — in der Reihenfolge des
/// Zaehlfeldes. Nur Diagnose (`--features strict-probe`).
pub const STRICT_SITE_NAMES: [&str; STRICT_SITES] = [
    "set: Empfaenger ist ein echtes Primitiv (Text/Zahl/Bool/Symbol)",
    "set: eigene Eigenschaft nicht schreibbar",
    "set: geerbte Eigenschaft nicht schreibbar",
    "set: nur Getter, kein Setzer",
    "set: Objekt nicht erweiterbar",
    "set: die Stellvertreter-Falle sagte nein",
    "Zuweisung an einen unbekannten Namen legt eine globale an",
    "delete gab false zurueck",
    "this ist undefined in einem einfachen Aufruf (locker: globalThis)",
    "this bleibt ein Primitiv (locker: eingepackt)",
    "eval legt sein var im Bereich des Aufrufers ab",
    // Getrennt vom echten Primitiv, und das ist keine Feinheit: fast jeder
    // Treffer hier heisst „das Objekt gibt es in beak gar nicht", also
    // `undefined.foo = 1` aus `propertyHelper.js`. Zusammengezaehlt haetten
    // die beiden Faelle die Rangliste angefuehrt und dabei eine ganz andere
    // Luecke gemessen als die, die sie zu messen vorgeben.
    "set: Empfaenger ist undefined/null (meist: das Objekt fehlt ganz)",
];
pub const STRICT_SITES: usize = 12;

/// Eine Stelle melden. Ohne die Fahne ist es nichts — kein Feld, kein Befehl.
#[cfg(feature = "strict-probe")]
macro_rules! strict_site {
    ($me:expr, $i:expr) => {{ $me.strict_probe[$i] += 1; }};
}
#[cfg(not(feature = "strict-probe"))]
macro_rules! strict_site {
    ($me:expr, $i:expr) => {{ let _ = &$me; }};
}
pub(crate) use strict_site;

/// Was eine Seite am Verlauf verlangt hat. **Eine Absicht, keine Tat** —
/// ausgefuehrt wird sie vom Wirt, der als einziger einen Verlauf hat.
#[derive(Debug, Clone)]
pub enum HistoryOp {
    /// `pushState(state, title, url)` — `url` ist bereits aufgeloest oder
    /// leer, wenn die Seite keine angab.
    Push { url: String },
    /// `replaceState(...)` — derselbe Eintrag, neue Adresse.
    Replace { url: String },
    /// `go(n)`, `back()` (= `go(-1)`), `forward()` (= `go(1)`).
    Go(i32),
}

pub struct Interp {
    pub realm: Realm,
    /// Die geholten ES-Module, nach AUFGELOESTER Adresse. Siehe `modules.rs`
    /// — die Engine holt nichts, sie verwaltet nur.
    pub modules: HashMap<Rc<str>, Rc<RefCell<super::modules::Module>>>,
    /// Welches Modul zuerst geworfen hat. Ein Fehler aus einem Graphen von
    /// sechsundfuenfzig Adressen nennt sonst nur den EINSTIEG, und das ist
    /// die eine Auskunft, die man nicht braucht.
    pub module_fail: Option<Rc<str>>,
    /// Stilblaetter, die ein SKRIPT eingehaengt hat und die noch geholt
    /// werden muessen: `(Knoten, Adresse wie im Attribut)`.
    ///
    /// Der Wirt holt sie mit `take_pending_sheets` ab, loest auf, laedt, und
    /// meldet den Ausgang mit `sheet_done` zurueck — dann erst faellt `load`
    /// oder `error` am `<link>`. Ohne diesen Weg wartet jede Seite, die ihr
    /// Blatt per Skript nachlaedt, fuer immer auf ein Ereignis, das nie kommt.
    pub pending_sheets: Vec<(u32, String)>,
    /// Formulare, die die SEITE abschicken will (`form.submit()`), als
    /// `seq` des `<form>`. Der Wirt holt sie mit `take_submits` ab und
    /// navigiert — die Engine kann das nicht, sie kennt weder Adresse noch
    /// Netz. Dasselbe Muster wie `history_ops`.
    pub submits: Vec<u32>,
    /// Abgelehnte Versprechen, an denen (noch) nichts haengt.
    pub pending_rejections: Vec<Gc>,
    /// `customElements`: Marke -> Konstruktor, in Reihenfolge der Anmeldung.
    ///
    /// Eine LISTE und keine Tabelle: sie wird bei jedem `new` einmal
    /// durchlaufen, um aus dem Prototyp die Marke zu finden, und vierzehn
    /// Eintraege sind kein Fall fuer eine Hashtabelle.
    pub custom: Vec<(Rc<str>, Value)>,
    /// Aufruftiefe. Ein Baumlaeufer benutzt den RUST-Stapel, also wird ein
    /// zu tiefes JS-Programm zum Stapelueberlauf des Wirts — und das ist im
    /// Kernel ein Absturz, kein Fehler. Die Grenze ist deshalb Pflicht, nicht
    /// Komfort.
    pub depth: usize,
    pub max_depth: usize,
    /// Ausgefuehrte Anweisungen. Ohne Deckel haengt ein `while(true)` den
    /// ganzen Lauf auf — und ein Testlaeufer, der an EINEM Programm stehen
    /// bleibt, misst gar nichts mehr.
    pub steps: u64,
    pub max_steps: u64,
    /// „Darf noch weitergerechnet werden?" — der Wirt setzt sie, die Engine
    /// fragt sie alle 65 536 Schritte.
    ///
    /// **Ein Schrittdeckel misst das Falsche.** Er sollte eine Seite stoppen,
    /// die sich aufhaengt, aber er trifft genauso eine, die viel RECHNET: die
    /// Anmeldung einer Fritzbox rechnet 66 000 PBKDF2-Runden, und die sind
    /// keine Endlosschleife. Ein Browser misst deshalb ZEIT, nicht Schritte.
    /// Die Engine hat keine Uhr — also fragt sie den Wirt.
    ///
    /// Alle 65 536 Schritte, nicht bei jedem: der Aufruf selbst darf den
    /// heissesten Pfad der Maschine nicht kosten.
    pub deadline: Option<fn() -> bool>,
    /// Eine Uhr, die nur steigt. Ersatz, bis beak die echte einreicht —
    /// `beak-engine` ist hostfrei und hat keine.
    pub fake_now: f64,
    /// Millisekunden seit der Epoche, wie sie der Wirt beim Sitzungsbeginn
    /// gesetzt hat. Die Engine selbst hat keine Uhr; ohne diesen Wert steht
    /// `Date.now()` bei 1970 — richtig, aber nutzlos.
    pub epoch_ms: f64,
    /// Das Dokument, auf dem `document` arbeitet. `None`, solange keins
    /// eingereicht wurde — dann gibt es `document` gar nicht erst, statt eins
    /// vorzutaeuschen, das nichts enthaelt.
    pub doc: Option<super::dombind::Doc>,
    /// Der Zustand von `Math.random`.
    ///
    /// **Der Wirt saet, nicht die Engine.** `beak-engine` ist hostfrei und hat
    /// keine Entropiequelle; sich eine auszudenken waere schlimmer als keine
    /// zu haben. Also steht hier eine feste Saat, und wer eine echte hat,
    /// reicht sie mit `seed_random` ein. Der Testlaeufer bekommt dadurch
    /// nebenbei, was er ohnehin braucht: reproduzierbare Laeufe.
    rng: u64,
    /// Die Medienlage fuer `matchMedia` — Breite und Farbschema-Wunsch. Wie
    /// `innerWidth` gehoert sie dem Wirt; ohne `set_viewport` gibt es
    /// `matchMedia` gar nicht erst.
    pub media: Option<(f64, bool)>,
    /// Was `getComputedStyle` braucht: das Blatt, der Baum, aus dem das
    /// Dokument gebaut wurde, das Farbschema und die Fensterbreite.
    ///
    /// **Der Wirt reicht es ein**, wie die Fenstergroesse und die Kekse. Die
    /// Maschine hat kein Stilblatt und soll keins holen; sie bekommt eins,
    /// wenn jemand eins hat. Ohne diesen Kontext antwortet
    /// `getComputedStyle` weiter aus dem Inline-Stil — eine Teilantwort, die
    /// die Seite laufen laesst, statt sie mit einem TypeError zu beenden.
    pub style_ctx: Option<StyleCtx>,
    /// Wieviele Programme die BEFEHLSMASCHINE gefahren hat und wieviele der
    /// Baumlaeufer — die Zahl, an der die Umstellung gemessen wird. Sie soll
    /// steigen, waehrend die test262-Zahl STEHEN BLEIBT: das eine ist der
    /// Fortschritt, das andere das Netz.
    pub vm_ran: u64,
    pub vm_declined: u64,
    /// Warum der Uebersetzer beim letzten Mal abgesagt hat. Ein Name, kein
    /// Satz — er wird gezaehlt, und eine Zaehlung braucht einen Schluessel.
    pub vm_decline: Option<&'static str>,
    /// Aus. Nur fuer die Gegenprobe: derselbe Lauf einmal MIT und einmal OHNE
    /// die Befehlsmaschine sagt in einem Diff, welche Tests sie verliert. Ohne
    /// diesen Schalter muesste man den Unterschied erraten, und beim ersten
    /// Lauf waren es 62 Tests von 69 194 — die findet man nicht durch Lesen.
    pub vm_off: bool,
    /// Uebersetzte Funktionsrumpfe, nach der Adresse ihres AST-Knotens.
    ///
    /// Ein `Rc<Func>` ist die Identitaet einer Funktion im Quelltext; derselbe
    /// Rumpf wird bei jedem Aufruf gebraucht und darf nur EINMAL uebersetzt
    /// werden. `None` heisst „schon versucht, geht nicht" — auch das gehoert
    /// gemerkt, sonst uebersetzt eine Schleife bei jedem Umlauf vergeblich.
    pub func_chunks: HashMap<usize, Option<Rc<super::code::Chunk>>>,
    /// Woran der Uebersetzer bei einem FUNKTIONSRUMPF absagt, je Grund.
    ///
    /// Die Programm-Absagen (`vm_decline`) waren bis Stufe 4 die ganze
    /// Rangliste — und sind es seitdem nicht mehr: ein Generator- oder
    /// async-Rumpf sagt ab, ohne dass das Programm darum absagt, und die
    /// Absage war damit UNSICHTBAR. Eine Rangfolge, die den halben Korpus
    /// nicht sieht, ist keine.
    pub func_declines: HashMap<&'static str, u64>,
    /// Die Marken, die zur naechsten Schleife gehoeren.
    ///
    /// `outer: for (…)` ist im Baum eine Marke UM eine Schleife; ein
    /// `continue outer` gehoert aber der SCHLEIFE — nur sie hat einen
    /// Fortsetzungspunkt. Also legt die Marke den Namen hier ab und die
    /// Schleife holt ihn beim Betreten. Ohne das lief ein `continue lbl` dem
    /// Baumlaeufer durch bis nach oben und beendete das Programm STILL.
    pub pending_labels: Vec<String>,
    /// Aufrufe, die als RAHMEN liefen, und solche, die ueber den Rust-Stapel
    /// mussten. Die zweite Zahl ist das, was Stufe 4 (Anhalten) noch im Weg
    /// steht: was ueber Rust laeuft, kann nicht stehenbleiben.
    pub vm_calls: u64,
    /// Eingebaute, gebundene, Getter — die haben keinen Rumpf aus Befehlen
    /// und werden nie einen haben. Sie gehoeren NICHT in denselben Nenner wie
    /// eine JS-Funktion, die der Uebersetzer bloss noch nicht kann.
    pub vm_calls_native: u64,
    pub vm_calls_slow: u64,
    /// Siehe `Geometry`. `None` heisst: der Wirt hat keine eingereicht, und
    /// dann antwortet die Geometrie mit Nullen wie eh und je.
    pub geometry: Option<Geometry>,
    /// Der lebende Baum in der Form, die die Kaskade lesen kann — gebaut aus
    /// `doc`, und nur neu gebaut, wenn `doc.version` sich bewegt hat.
    ///
    /// Frueher hielt `StyleCtx` einen SCHNAPPSCHUSS vom Skriptstart, und
    /// `getComputedStyle` antwortete daraus. Ein Skript, das eine Klasse setzt
    /// und dann misst, bekam den Stand von vorher — zwei Antworten auf
    /// dieselbe Frage. Jetzt gibt es nur noch einen Baum.
    pub live_dom: core::cell::RefCell<Option<(u32, alloc::rc::Rc<crate::dom::Dom>)>>,
    /// Die Kekse dieser Seite, so wie `document.cookie` sie zeigt.
    ///
    /// **Der Wirt reicht sie ein, die Engine hat keinen Behaelter.** Der
    /// Behaelter kennt Domain, Pfad, `Secure` und `HttpOnly`; welche davon
    /// dieses Dokument sehen darf, ist eine Frage an ihn und nicht an die
    /// Maschine. `None` heisst „niemand hat gefragt" — dann gibt es
    /// `document.cookie` trotzdem, als leere Zeichenkette, weil ein Skript
    /// darauf `.match` ruft und ein `undefined` es toetet. Das ist keine
    /// erfundene Antwort: keine Kekse IST eine Antwort.
    /// **Nur mit `--features strict-probe`.** Zaehlt die Stellen, an denen der
    /// strenge Modus etwas anderes taete als der lockere — jede fuer sich.
    ///
    /// Die Fahnen der Tests (`onlyStrict`) beantworten die Frage NICHT: sie
    /// sagen, wie ein Test GESTARTET wird, nicht, ob er an einer dieser
    /// Stellen vorbeikommt. Ein Test ohne Fahne, der in seinem Rumpf
    /// `"use strict"` schreibt, haengt genauso daran.
    #[cfg(feature = "strict-probe")]
    pub strict_probe: [u32; STRICT_SITES],
    pub cookies: String,
    /// Was die Seite mit `document.cookie = "…"` gesetzt hat, roh und in der
    /// Reihenfolge. Der Wirt holt es sich mit `take_cookie_sets` und legt es
    /// in seinen Behaelter — die Engine entscheidet nicht, was gilt.
    pub cookie_sets: Vec<String>,
    /// Was die Seite mit `history.pushState`/`replaceState`/`go` verlangt hat
    /// — roh und in der Reihenfolge. **Die Engine navigiert nicht**, sie hat
    /// keinen Verlauf und soll keinen erfinden; sie sammelt, und der Wirt
    /// holt es mit `take_history_ops` ab und entscheidet. Dasselbe Muster
    /// wie bei den Keksen.
    pub history_ops: Vec<HistoryOp>,
    /// `history.state` — der Zustand, den die Seite zuletzt gesetzt hat.
    /// Er gehoert dem DOKUMENT, nicht dem Verlauf des Wirts, und lebt
    /// deshalb hier.
    pub history_state: Value,
    /// `history.length`, vom Wirt eingereicht. Ohne ihn steht 1 da — ein
    /// frisch geladenes Dokument ist immer mindestens ein Eintrag.
    pub history_len: f64,
    /// Laufende Nummer fuer `Symbol()`.
    pub next_sym: u32,
    /// Die globale Symbolregistrierung hinter `Symbol.for`/`Symbol.keyFor`.
    pub sym_registry: HashMap<Rc<str>, Value>,
    /// Die Microtask-Schlange. Sie steht NEBEN `timers`, nicht darin: eine
    /// Microtask laeuft vor dem naechsten Zeitgeber, nicht danach — das ist
    /// der ganze Unterschied zwischen `Promise.resolve().then(f)` und
    /// `setTimeout(f, 0)`.
    pub jobs: alloc::collections::VecDeque<super::promise::Job>,
    /// Angemeldete Zeitgeber-Rueckrufe. Noch laeuft niemand sie; sie zu HALTEN
    /// kostet nichts und ist die Stelle, an der beaks Schleife ansetzt.
    pub timers: Vec<Value>,
    /// Was die Seite auf `console` geschrieben hat.
    ///
    /// Gesammelt statt weggeworfen: `beak-engine` hat keine Serienleitung,
    /// aber der Wirt hat eine, und eine Seite, die ihren eigenen Zustand
    /// meldet, ist bei einer Ferndiagnose oft das einzige Fenster hinein.
    /// Gedeckelt, weil eine fremde Seite sonst den Speicher damit fuellt —
    /// und der Verlust wird gemeldet, nicht verschwiegen.
    /// Der letzte erfolgreiche Treffer — allein fuer die annexB-Statiken
    /// `RegExp.$1`, `RegExp.lastMatch` & Co. Sie stehen NICHT am
    /// Ausdrucksobjekt, sondern am Konstruktor, also muss der Zustand hier
    /// liegen und nicht dort.
    /// Laeuft gerade ein `new` auf einem eingebauten Konstruktor? Ein
    /// natives `this` ist bei Aufruf und Bau dasselbe (`undefined`), also
    /// braucht es diese Fahne — `Symbol` HAT ein `[[Construct]]`, es wirft
    /// nur darin.
    pub native_new: bool,
    pub last_match: Option<LastMatch>,
    pub console: Vec<String>,
    console_dropped: usize,
}

/// Wie viele Zeilen `console` haelt, und wie lang eine werden darf.
/// Was die neun `RegExp.$n` und ihre vier Nachbarn brauchen. Fertige
/// Zeichenketten statt Bereiche: die Quelle darf danach verschwinden.
pub struct LastMatch {
    pub input: String,
    pub matched: String,
    pub left: String,
    pub right: String,
    /// `$1` bis `$9`; eine Gruppe ohne Treffer ist die leere Zeichenkette.
    pub caps: Vec<String>,
    pub last_paren: String,
}

pub const MAX_CONSOLE_LINES: usize = 200;
pub const MAX_CONSOLE_LEN: usize = 512;

pub const MAX_DEPTH: usize = 400;

/// Wie weit eine Prototypkette laufen darf.
///
/// **Ein Sicherungsnetz, keine Regel der Sprache.** Eine Kette kann einen
/// Zyklus enthalten (`Object.setPrototypeOf` muss ihn zwar ablehnen, aber
/// darauf allein soll sich hier nichts verlassen), und dann laeuft jeder
/// Eigenschaftszugriff fuer immer — in NATIVEM Code, an der Schrittgrenze
/// vorbei. Eine fremde Seite haette damit drei Zeilen gebraucht, um beak
/// aufzuhaengen. Echte Ketten sind ein Dutzend Glieder tief.
pub const MAX_PROTO_CHAIN: usize = 1000;

/// Wieviele Bytes ein einzelner `ArrayBuffer` haben darf.
///
/// Keine erfundene Grenze, sondern die Antwort auf eine echte: der Lauf ist
/// an `new ArrayBuffer(2**53)` gestorben, und in einem Kernel gibt es keinen
/// Prozess, der dabei alleine stirbt. 64 MB sind mehr, als jede Seite im
/// Zielkorpus je in einem Stueck belegt, und der Fehlschlag ist ein
/// `RangeError` — genau der, den ein echter Motor bei gescheiterter Zuteilung
/// gibt.
pub const MAX_BUFFER_BYTES: usize = 64 << 20;

/// Was ein Testlaeufer setzt.
///
/// Gegen eine Messung gesetzt, nicht gegen ein Gefuehl: ein gewoehnlicher
/// test262-Test kostet **1,9 µs**, also grob hundert Schritte. 2 Mio. waren
/// das Zwanzigtausendfache — und weil diese Maschine rund 11 Mio. Schritte je
/// Sekunde schafft, kostete JEDER Test, der absichtlich mit einer absurden
/// Array-Laenge arbeitet, 180 ms. Davon gibt es in `built-ins/Array` Tausende.
///
/// 200 000 sind immer noch hundertfache Reserve und decken hoechstens 18 ms.
/// Was darueber faellt, verschwindet nicht still: „step budget exhausted"
/// steht als eigene Zeile in der Fehlerkarte.
pub const TEST_STEPS: u64 = 200_000;

impl Interp {
    pub fn new() -> Interp {
        let mut realm = super::builtins::make_realm();
        super::dombind::install(&mut realm);
        super::regexp::install(&mut realm);
        super::json::install(&mut realm);
        super::promise::install(&mut realm);
        super::date::install(&mut realm);
        super::iterhelp::install(&mut realm);
        super::proxy::install(&mut realm);
        realm.eval_fn = match realm.global.borrow().get_own("eval").and_then(|p| p.value.clone()) {
            Some(Value::Obj(o)) => Some(o), _ => None,
        };
        super::url::install(&mut realm);
        Interp { realm, modules: HashMap::new(), module_fail: None, submits: Vec::new(),
                 deadline: None,
                 pending_sheets: Vec::new(),
                 pending_rejections: Vec::new(), custom: Vec::new(), depth: 0, max_depth: MAX_DEPTH, steps: 0, max_steps: u64::MAX,
                 fake_now: 0.0, epoch_ms: 0.0, doc: None, next_sym: 0, sym_registry: HashMap::new(),
                 #[cfg(feature = "strict-probe")]
                 strict_probe: [0; STRICT_SITES],
                 cookies: String::new(), cookie_sets: Vec::new(), style_ctx: None,
                 history_ops: Vec::new(), history_state: Value::Null, history_len: 1.0,
                 vm_ran: 0, vm_declined: 0, vm_decline: None, vm_off: false,
                 func_chunks: HashMap::new(), func_declines: HashMap::new(), pending_labels: Vec::new(), vm_calls: 0, vm_calls_native: 0, vm_calls_slow: 0,
                 geometry: None,
                 live_dom: core::cell::RefCell::new(None),
                 jobs: alloc::collections::VecDeque::new(),
                 rng: 0x2545_F491_4F6C_DD1D, media: None,
                 timers: Vec::new(), native_new: false, last_match: None, console: Vec::new(), console_dropped: 0 }
    }

    /// Die angemeldeten Zeitgeber EINMAL durchlaufen.
    ///
    /// Einmal, nicht bis die Schlange leer ist: ein `setTimeout`, das sich
    /// selbst neu anmeldet, ist ein voellig normales Muster (Abfrageschleifen,
    /// Animationen) und wuerde die Schleife sonst nie verlassen. Was waehrend
    /// des Laufs dazukommt, ist beim naechsten Mal dran.
    pub fn run_timers(&mut self) -> usize {
        // Erst die Microtasks, dann die Zeitgeber — das IST die Rangfolge.
        // Und ohne diese Zeile bliebe ein `Promise.resolve().then(f)` aus
        // einem Ereignisbehandler liegen, bis zufaellig ein Zeitgeber faellig
        // wird: `run_timers` kaeme bei leerer Zeitgeberliste gar nicht dazu.
        super::promise::run_jobs(self);
        let due = core::mem::take(&mut self.timers);
        let n = due.len();
        for f in due {
            // Ein Zeitgeber, der wirft, muss es SAGEN. Der Ausgang wurde hier
            // weggeworfen: ein Fehler in einem `setTimeout`-Rueckruf war
            // unsichtbar, und was danach nicht passierte, sah aus wie ein
            // fehlendes Merkmal — dieselbe Falle wie beim Ereignisbehandler.
            if let Err(e) = self.call(&f, Value::Undefined, &[]) {
                let msg = super::modules::describe(self, e);
                self.console_push(alloc::format!("Fehler im Zeitgeber: {msg}"));
            }
            // Nach JEDEM Zeitgeber, nicht erst nach allen: Microtasks laufen
            // zwischen den Aufgaben, und ein `then`, das der erste Zeitgeber
            // anlegt, gehoert vor den zweiten.
            super::promise::run_jobs(self);
        }
        n
    }

    /// Ein Dokument einreichen und `document` global sichtbar machen.
    ///
    /// Den Realm abbauen und dabei die Ringe brechen.
    ///
    /// **Gemessen 2026-09-04: ein Realm kostet 973 KB und wurde NIE frei.**
    /// Zweihundert erzeugte und wieder fallengelassene Maschinen liessen
    /// 191 MB liegen. Der Grund ist kein Fehler in einer Zeile, sondern die
    /// Bauart: `Rc` zaehlt, und die Form eines JS-Realms ist ringfoermig —
    /// `proto.constructor` zeigt auf den Konstruktor, `ctor.prototype`
    /// zurueck auf den Prototyp; `globalThis` zeigt auf sich selbst; jede
    /// Schliessung haelt ihre Umgebung, und die globale Umgebung haelt die
    /// Schliessung. Ein Zaehler kommt aus einem Ring nie auf null.
    ///
    /// Der test262-Lauf hat es aufgedeckt: 69 194 Tests x 973 KB = 67 GB, und
    /// der OOM-Killer hat den Lauf erschossen. Bei 519 KB (0.56.0) passte es
    /// gerade noch — die Luecke war also schon lange da, nur nicht sichtbar.
    ///
    /// Kein Sammler, sondern ein ABBAU an den Wurzeln: alles, was vom
    /// globalen Gegenstand und den Prototypen aus erreichbar ist, wird
    /// einmal besucht und geleert. Danach zeigt kein Ring mehr auf sich
    /// selbst, und `Rc` raeumt den Rest.
    ///
    /// ⚠ Wer nach dem Fallenlassen noch einen `Value` aus dieser Maschine
    /// haelt, haelt danach ein LEERES Objekt. Das ist sicher (der Speicher
    /// lebt, solange der `Rc` lebt), aber es ist nicht mehr dasselbe Objekt.
    fn teardown(&mut self) {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut envs: HashSet<usize> = HashSet::new();
        let mut objs: Vec<Gc> = alloc::vec![self.realm.global.clone()];
        let mut env_stack: Vec<Rc<RefCell<Env>>> = alloc::vec![self.realm.global_env.clone()];
        // Die Prototypen stehen im Realm und sind nicht immer vom globalen
        // Gegenstand aus erreichbar (`event_proto` etwa haengt am
        // Konstruktor, aber der Umweg ist nicht garantiert).
        objs.extend(self.realm.roots());
        let mut all: Vec<Gc> = Vec::new();
        while let Some(o) = objs.pop() {
            if !seen.insert(Rc::as_ptr(&o) as usize) { continue }
            {
                let b = o.borrow();
                if let Some(p) = &b.proto { objs.push(p.clone()); }
                for k in b.own_keys() {
                    let Some(pr) = b.get_own(&k) else { continue };
                    for v in [&pr.value, &pr.get, &pr.set].into_iter().flatten() {
                        if let Value::Obj(x) = v { objs.push(x.clone()); }
                    }
                }
                match &b.kind {
                    ObjKind::Function(d) => {
                        env_stack.push(d.env.clone());
                        if let Some(Value::Obj(x)) = &d.this_val { objs.push(x.clone()); }
                        if let Some(h) = &d.home_object { objs.push(h.clone()); }
                    }
                    // Ein angehaltener Generator haelt seine Umgebungen und
                    // halbfertige Werte in seiner Maschine fest. Sie stehen
                    // sonst in keiner Eigenschaft und in keiner Bindung — wer
                    // sie hier auslaesst, laesst einen Rc-Ring stehen.
                    ObjKind::Generator(g) => {
                        g.roots(&mut objs, &mut env_stack);
                    }
                    ObjKind::Bound { target, this_val, args } => {
                        objs.push(target.clone());
                        if let Value::Obj(x) = this_val { objs.push(x.clone()); }
                        for a in args { if let Value::Obj(x) = a { objs.push(x.clone()); } }
                    }
                    _ => {}
                }
            }
            all.push(o);
        }
        // Die Umgebungen dazu: eine Schliessung haelt ihre, und die haelt
        // ueber ihre Bindungen wieder Schliessungen.
        let mut all_envs: Vec<Rc<RefCell<Env>>> = Vec::new();
        while let Some(e) = env_stack.pop() {
            if !envs.insert(Rc::as_ptr(&e) as usize) { continue }
            {
                let b = e.borrow();
                if let Some(p) = &b.parent { env_stack.push(p.clone()); }
                for v in b.vars.values() {
                    if let Value::Obj(x) = &v.value {
                        if seen.insert(Rc::as_ptr(x) as usize) { all.push(x.clone()); }
                        // Objekte aus Umgebungen koennen selbst Umgebungen
                        // halten — deshalb dieselbe Behandlung.
                        if let ObjKind::Function(d) = &x.borrow().kind { env_stack.push(d.env.clone()); }
                    }
                }
            }
            all_envs.push(e);
        }
        for o in &all {
            let mut b = o.borrow_mut();
            b.clear_props();
            b.proto = None;
            b.kind = ObjKind::Plain;
        }
        for e in &all_envs {
            let mut b = e.borrow_mut();
            b.vars.clear();
            b.parent = None;
            b.this_val = None;
            b.home = None;
        }
    }

    /// Erst hier entsteht `document` — vorher gibt es den Namen nicht, und ein
    /// Skript, das ihn prueft, bekommt die Wahrheit statt eine leere Huelle.
    pub fn set_document(&mut self, doc: super::dombind::Doc) {
        let root = doc.doc;
        self.doc = Some(doc);
        let v = super::dombind::wrap(self, root);
        self.realm.global.borrow_mut().define("document", Prop::builtin(v));
    }

    /// Eine Zeile von der Seite entgegennehmen.
    pub fn console_push(&mut self, line: String) {
        if self.console.len() >= MAX_CONSOLE_LINES {
            self.console_dropped += 1;
            return;
        }
        let mut l = line;
        if l.len() > MAX_CONSOLE_LEN {
            l.truncate(MAX_CONSOLE_LEN);
            l.push_str(" …");
        }
        self.console.push(l);
    }

    /// Die gesammelten Zeilen herausnehmen. Wurde etwas verworfen, sagt die
    /// letzte Zeile es — sonst laese sich eine gedeckelte Ausgabe wie eine
    /// vollstaendige.
    pub fn take_console(&mut self) -> Vec<String> {
        let mut out = core::mem::take(&mut self.console);
        if self.console_dropped > 0 {
            out.push(alloc::format!("… {} weitere Zeilen verworfen", self.console_dropped));
            self.console_dropped = 0;
        }
        out
    }

    /// Die Fenstergroesse einreichen.
    ///
    /// `beak-engine` hat keine — sie gehoert dem Wirt. Vorher gab es
    /// `innerWidth` deshalb GAR NICHT, und eine Seite, die ihr Layout danach
    /// waehlt, fiel mit `ReferenceError` aus, statt die schmale Fassung zu
    /// nehmen. Eine erfundene Zahl waere schlimmer gewesen: sie haette
    /// ausgesehen wie eine Messung ([[feedback_invented_fallback_hides_the_fault]]).
    /// Eine echte Saat vom Wirt. Ohne sie liefert `Math.random` jedes Mal
    /// dieselbe Folge — sichtbar deterministisch statt unsichtbar schlecht.
    pub fn seed_random(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    /// xorshift64*. Eine Zahl in [0,1), wie die Spezifikation sie verlangt.
    /// Kein Kryptozufall und nicht als solcher gedacht — `crypto.getRandomValues`
    /// waere eine eigene Frage und haengt am Wirt.
    pub fn next_random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.rng = x;
        // Die oberen 53 Bits: genau die Genauigkeit eines f64-Bruchs.
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Die Kekse dieser Seite einreichen — was `document.cookie` LIEST.
    ///
    /// Der Wirt gibt die Skript-Sicht (`cookies::script_header_for`), nicht
    /// den `Cookie:`-Kopf: ein `HttpOnly`-Keks reist auf der Anfrage mit und
    /// darf trotzdem nie in einem Skript stehen.
    pub fn set_cookies(&mut self, jar: String) {
        self.cookies = jar;
    }

    /// Was die Seite gesetzt hat, herausnehmen. Rohe Erklaerungen
    /// (`name=wert; Path=/; Max-Age=…`) — die Regeln kennt der Behaelter.
    /// Was die Seite am Verlauf tun WOLLTE. Der Wirt holt es ab und
    /// entscheidet; die Liste ist danach leer.
    pub fn take_pending_sheets(&mut self) -> Vec<(u32, String)> {
        core::mem::take(&mut self.pending_sheets)
    }

    pub fn take_submits(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.submits)
    }

    pub fn take_history_ops(&mut self) -> Vec<HistoryOp> {
        core::mem::take(&mut self.history_ops)
    }

    /// Der Wirt reicht ein, wie lang sein Verlauf ist und welchen Zustand
    /// der aktuelle Eintrag traegt — beim Laden und nach jedem Sprung.
    pub fn set_history(&mut self, len: f64, state: Value) {
        self.history_len = len;
        self.history_state = state;
    }

    pub fn take_cookie_sets(&mut self) -> Vec<String> {
        core::mem::take(&mut self.cookie_sets)
    }

    /// Den Kaskadenkontext einreichen — damit `getComputedStyle` echte Werte
    /// liefern kann statt nur des Inline-Stils.
    pub fn set_style_context(&mut self, ctx: StyleCtx) {
        self.style_ctx = Some(ctx);
    }

    /// Die Kaesten des letzten Layouts einreichen — siehe `Geometry`.
    /// Der Rollstand aendert sich ohne Layout, also gehoert er MIT hinein und
    /// wird bei jedem Einreichen nachgezogen.
    pub fn set_geometry(&mut self, g: Geometry) {
        self.geometry = Some(g);
    }

    /// Die Adresse der Seite einreichen. Fuellt `location` und `document.URL`.
    ///
    /// Vorher stand dort `about:blank` — eine Zahl, die aussieht wie eine
    /// Messung: ein Skript, das seinen Pfad prueft, nahm den falschen Zweig
    /// und meldete keinen Fehler dabei.
    pub fn set_location(&mut self, url: &str) {
        let p = super::url::parse_abs(url).unwrap_or_else(|| super::url::Parts {
            scheme: alloc::string::String::from("about"),
            path: alloc::string::String::from("blank"),
            ..Default::default()
        });
        let loc = match self.realm.global.borrow().get_own("location").and_then(|p| p.value.clone()) {
            Some(Value::Obj(o)) => o,
            _ => return,
        };
        let mut o = loc.borrow_mut();
        for (k, v) in [("href", p.href()), ("protocol", alloc::format!("{}:", p.scheme)),
                       ("host", p.host_with_port()), ("hostname", p.host.clone()),
                       ("port", p.port.clone()), ("origin", p.origin()),
                       ("pathname", p.path.clone()),
                       ("search", if p.query.is_empty() { String::new() } else { alloc::format!("?{}", p.query) }),
                       ("hash", if p.hash.is_empty() { String::new() } else { alloc::format!("#{}", p.hash) })] {
            o.define(k, Prop::builtin(Value::str(&v)));
        }
        drop(o);
        let href = p.href();
        self.realm.global.borrow_mut().define("origin", Prop::builtin(Value::str(&p.origin())));
        if let Some(Value::Obj(d)) = self.realm.global.borrow().get_own("document").and_then(|p| p.value.clone()) {
            let mut d = d.borrow_mut();
            d.define("URL", Prop::builtin(Value::str(&href)));
            d.define("documentURI", Prop::builtin(Value::str(&href)));
            d.define("location", Prop::builtin(Value::Obj(loc.clone())));
        }
    }

    /// Wie `set_viewport`, aber mit dem Farbschema dazu. `matchMedia` braucht
    /// beides, und ein `prefers-color-scheme`, das immer hell sagt, waere eine
    /// erfundene Antwort.
    pub fn set_media(&mut self, w: f64, h: f64, dark: bool) {
        self.set_viewport(w, h);
        self.media = Some((w, dark));
    }

    pub fn set_viewport(&mut self, w: f64, h: f64) {
        if self.media.is_none() { self.media = Some((w, false)); }
        let g = self.realm.global.clone();
        let mut o = g.borrow_mut();
        for (k, v) in [("innerWidth", w), ("innerHeight", h),
                       ("outerWidth", w), ("outerHeight", h),
                       ("scrollX", 0.0), ("scrollY", 0.0), ("devicePixelRatio", 1.0)] {
            o.define(k, Prop::builtin(Value::Num(v)));
        }
        let screen = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut sc = screen.borrow_mut();
            for (k, v) in [("width", w), ("height", h), ("availWidth", w), ("availHeight", h)] {
                sc.define(k, Prop::builtin(Value::Num(v)));
            }
            sc.define("colorDepth", Prop::builtin(Value::Num(24.0)));
            sc.define("pixelDepth", Prop::builtin(Value::Num(24.0)));
        }
        o.define("screen", Prop::builtin(Value::Obj(screen)));
    }

    /// Ein Arbeitsschritt in einer EINGEBAUTEN Schleife.
    ///
    /// Der Deckel in `exec` zaehlt nur Anweisungen — die Schleifen in
    /// `Array.prototype.*` und `iterate` laufen daran vorbei. `new
    /// Array(2**32-1).join()` haengt damit unbegrenzt, und genau das hat den
    /// ersten Ausfuehrungslauf ueber den Zeitdeckel getragen. Also zaehlen
    /// diese Schleifen mit.
    pub fn tick(&mut self) -> C<()> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(self.throw_kind("RangeError", "step budget exhausted"));
        }
        if self.steps & 0xFFFF == 0 {
            if let Some(f) = self.deadline {
                if !f() {
                    return Err(self.throw_kind("RangeError", "script ran too long"));
                }
            }
        }
        Ok(())
    }

    // ── Fehler ───────────────────────────────────────────────────────────
    pub fn throw_kind(&mut self, kind: &'static str, msg: &str) -> Abrupt {
        let proto = self.realm.error_ctors.get(kind).cloned()
            .unwrap_or_else(|| self.realm.error_proto.clone());
        let e = new_kind(Some(proto), ObjKind::Error);
        e.borrow_mut().define("message", Prop::builtin(Value::str(msg)));
        Abrupt::Throw(Value::Obj(e))
    }
    pub fn type_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("TypeError", msg)) }

    /// „x is not a function" — mit dem NAMEN, wo es einen gibt.
    ///
    /// „value is not a function" war im Zielkorpus mit 46 Fehlschlaegen der
    /// haeufigste Grund ueberhaupt und sagte ueber keinen einzigen, WAS fehlt
    /// ([[feedback_print_the_identifier_not_just_the_event]]). Eigene
    /// Funktion, weil beide Maschinen sie brauchen: als `switch` und `for..in`
    /// uebersetzbar wurden, wanderten zwei Korpusskripte auf die Maschine —
    /// und verloren dabei still ihren Namen in der Meldung. Die Prozentzahl
    /// hat das nicht gesehen, der Wandvergleich schon.
    pub fn not_a_function(&mut self, name: Option<&str>) -> Abrupt {
        match name {
            Some(n) => self.throw_kind("TypeError", &alloc::format!("{n} is not a function")),
            None => self.throw_kind("TypeError", "value is not a function"),
        }
    }
    pub fn range_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("RangeError", msg)) }
    pub fn ref_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("ReferenceError", msg)) }

    // ── Umwandlungen ─────────────────────────────────────────────────────
    /// `ToPrimitive`. `hint_string` waehlt die Reihenfolge von `toString` und
    /// `valueOf` — das ist der ganze Unterschied zwischen `"" + obj` und
    /// `1 * obj`.
    pub fn to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {
        self.to_primitive_hint(v, if hint_string { "string" } else { "number" })
    }

    /// Mit dem DRITTEN Wunsch: `"default"`. Er unterscheidet sich fuer
    /// gewoehnliche Objekte in nichts von `"number"` — aber `Symbol.
    /// toPrimitive` bekommt ihn zu sehen, und ein `Date` macht daraus Text.
    /// Ohne ihn waere `date + ""` eine Zahl.
    pub fn to_primitive_hint(&mut self, v: &Value, hint: &str) -> C<Value> {
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        // `Symbol.toPrimitive` geht VOR `valueOf`/`toString` — es ist der
        // einzige Weg, auf dem ein Objekt beide ueberstimmen kann.
        let exotic = self.get(v, SYM_TO_PRIMITIVE)?;
        if self.is_callable(&exotic) {
            let r = self.call(&exotic, v.clone(), &[Value::str(hint)])?;
            if !matches!(r, Value::Obj(_)) { return Ok(r); }
            return self.type_err("Symbol.toPrimitive returned an object");
        }
        let _ = o;
        self.ordinary_to_primitive(v, hint == "string")
    }

    /// `OrdinaryToPrimitive` — `valueOf` und `toString` in der Reihenfolge,
    /// die der Wunsch vorgibt. Herausgezogen, weil `Date.prototype[Symbol.
    /// toPrimitive]` sie ruft: sonst waere sie dort ein zweites Mal
    /// geschrieben.
    pub fn ordinary_to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        let order: [&str; 2] = if hint_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for m in order {
            let f = self.get(&Value::Obj(o.clone()), m)?;
            if self.is_callable(&f) {
                let r = self.call(&f, v.clone(), &[])?;
                if !matches!(r, Value::Obj(_)) { return Ok(r); }
            }
        }
        self.type_err("cannot convert object to primitive value")
    }

    pub fn to_number(&mut self, v: &Value) -> C<f64> {
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            Value::Num(n) => *n,
            Value::Str(s) => string_to_num(s),
            Value::Sym(_) => return self.type_err("cannot convert a Symbol value to a number"),
            // Eine grosse Zahl wird NICHT still zu einer kleinen. Das ist der
            // ganze Sinn des Typs: `1n + 1` ist ein Fehler, keine 2.
            Value::BigInt(_) => return self.type_err("cannot convert a BigInt value to a number"),
            Value::Obj(_) => { let p = self.to_primitive(v, false)?; self.to_number(&p)? }
        })
    }

    /// `ToBigInt` — die Umwandlung, die eine 64-Bit-Sicht beim Schreiben
    /// verlangt. Eine gewoehnliche Zahl wirft: der Uebergang muss im
    /// Quelltext stehen.
    pub fn to_bigint(&mut self, v: &Value) -> C<super::bigint::Big> {
        let p = self.to_primitive(v, false)?;
        Ok(match &p {
            Value::BigInt(b) => (**b).clone(),
            Value::Bool(b) => super::bigint::Big::from_u64(if *b { 1 } else { 0 }),
            Value::Str(t) => match super::bigint::Big::parse(t) {
                Some(b) => b,
                None => return Err(self.throw_kind("SyntaxError", "cannot convert string to a BigInt")),
            },
            _ => return self.type_err("cannot convert value to a BigInt"),
        })
    }

    /// `ToNumeric` — eine grosse Zahl bleibt gross, alles andere wird eine
    /// gewoehnliche. Der Unterschied zu `to_number` ist genau der Grund,
    /// warum `x++` auf einem BigInt nicht `+x` sein darf.
    pub fn to_numeric(&mut self, v: &Value) -> C<Value> {
        let p = self.to_primitive(v, false)?;
        if matches!(p, Value::BigInt(_)) { return Ok(p); }
        Ok(Value::Num(self.to_number(&p)?))
    }

    /// Eins dazu oder eins weg, im TYP des Wertes.
    pub fn step_numeric(&mut self, v: &Value, up: bool) -> C<Value> {
        Ok(match v {
            Value::BigInt(b) => {
                let one = super::bigint::Big::from_u64(1);
                Value::BigInt(Rc::new(if up { b.add(&one) } else { b.sub(&one) }))
            }
            _ => { let n = self.to_number(v)?; Value::Num(if up { n + 1.0 } else { n - 1.0 }) }
        })
    }

    pub fn to_string(&mut self, v: &Value) -> C<Rc<str>> {
        Ok(match v {
            Value::Undefined => Rc::from("undefined"),
            Value::Null => Rc::from("null"),
            Value::Bool(b) => Rc::from(if *b { "true" } else { "false" }),
            Value::Num(n) => Rc::from(num_to_string(*n).as_str()),
            Value::Str(s) => s.clone(),
            // Absichtlich ein Fehler, kein Text. `"" + sym` ist fast immer ein
            // Versehen; `String(sym)` und `sym.toString()` gehen weiterhin,
            // die rufen `sym_to_display` statt hier durch.
            Value::Sym(_) => return self.type_err("cannot convert a Symbol value to a string"),
            Value::BigInt(b) => Rc::from(b.to_string_radix(10).as_str()),
            Value::Obj(_) => { let p = self.to_primitive(v, true)?; self.to_string(&p)? }
        })
    }

    /// `ToPropertyKey`. Der EINE Punkt, an dem ein Symbol zum
    /// Eigenschaftsnamen wird — jeder berechnete Zugriff (`o[k]`,
    /// Objektliteral, Klassenglied, `in`, `defineProperty`) laeuft hier
    /// durch, und nur hier.
    pub fn to_prop_key(&mut self, v: &Value) -> C<Rc<str>> {
        match v {
            Value::Sym(sd) => Ok(sd.key.clone()),
            _ => self.to_string(v),
        }
    }

    /// Wie ein Symbol GESCHRIEBEN aussieht: `Symbol(desc)`. Nicht `to_string`
    /// — das wirft mit Absicht.
    pub fn sym_to_display(sd: &SymData) -> Rc<str> {
        match &sd.desc {
            Some(d) => Rc::from(alloc::format!("Symbol({d})").as_str()),
            None => Rc::from("Symbol()"),
        }
    }

    /// Ein frisches Symbol. Die laufende Nummer macht den Schluessel einmalig
    /// — zwei `Symbol("x")` sind damit verschieden, wie die Spezifikation es
    /// verlangt.
    /// Die Beschreibung steht MIT im Schluessel — `Object.getOwnPropertySymbols`
    /// bekommt nur ihn zu sehen und muss das Symbol daraus wieder aufbauen
    /// koennen ([[sym_from_key]]). Die laufende Nummer davor haelt ihn
    /// einmalig, damit zwei `Symbol("x")` verschieden bleiben.
    pub fn new_symbol(&mut self, desc: Option<Rc<str>>) -> Value {
        self.next_sym += 1;
        let n = self.next_sym;
        let key: Rc<str> = Rc::from(match &desc {
            Some(d) => alloc::format!("\0#{n}:{d}"),
            None => alloc::format!("\0#{n}"),
        }.as_str());
        Value::Sym(Rc::new(SymData { desc, key, registered: None }))
    }

    /// `ToObject`: Primitive bekommen ihre Huelle. Das ist der Weg, ueber den
    /// `"abc".length` funktioniert.
    pub fn to_object(&mut self, v: &Value) -> C<Gc> {
        match v {
            Value::Obj(o) => Ok(o.clone()),
            Value::Str(s) => {
                let g = new_kind(Some(self.realm.string_proto.clone()), ObjKind::StrWrap(s.clone()));
                {
                    let mut b = g.borrow_mut();
                    b.define("length", Prop::frozen(Value::Num(s.chars().count() as f64)));
                    for (i, c) in s.chars().enumerate() {
                        let mut t = String::new(); t.push(c);
                        b.define(&num_to_string(i as f64), Prop {
                            value: Some(Value::string(t)), get: None, set: None,
                            writable: false, enumerable: true, configurable: false });
                    }
                }
                Ok(g)
            }
            Value::Sym(sd) => Ok(new_kind(Some(self.realm.symbol_proto.clone()), ObjKind::SymWrap(sd.clone()))),
            Value::Num(n) => Ok(new_kind(Some(self.realm.number_proto.clone()), ObjKind::NumWrap(*n))),
            Value::Bool(b) => Ok(new_kind(Some(self.realm.boolean_proto.clone()), ObjKind::BoolWrap(*b))),
            Value::BigInt(b) => Ok(new_kind(Some(self.realm.bigint_proto.clone()), ObjKind::BigWrap(b.clone()))),
            Value::Undefined | Value::Null =>
                self.type_err("cannot convert undefined or null to object"),
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        let Value::Obj(o) = v else { return false };
        let kind = &o.borrow().kind;
        match kind {
            ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => true,
            // Ein Stellvertreter ist aufrufbar, wenn sein ZIEL es ist —
            // `typeof new Proxy(f, {})` ist "function".
            ObjKind::Proxy(c) => match c.borrow().clone() {
                Some((t, _)) => self.is_callable(&Value::Obj(t)),
                None => false,
            },
            _ => false,
        }
    }

    /// Darf `new` darauf? Ein Pfeil, eine Methode, eine async-Funktion und
    /// ein Generator sind KEINE Konstruktoren — und `Reflect.construct` mit
    /// einem solchen als `newTarget` muss werfen. Genau daran haengt der
    /// `isConstructor`-Helfer von test262, den ein paar hundert Tests rufen.
    ///
    /// Benannt statt verschwiegen: eine Methodenkurzform (`{ m(){} }`) sieht
    /// in unserem Baum aus wie eine gewoehnliche Funktion und gilt hier
    /// deshalb faelschlich als Konstruktor.
    pub fn is_constructor(&self, v: &Value) -> bool {
        let Value::Obj(o) = v else { return false };
        let kind = &o.borrow().kind;
        match kind {
            ObjKind::Native(n) => n.ctor,
            ObjKind::Function(d) =>
                !d.node.is_arrow && !d.node.is_async && !d.node.is_generator,
            ObjKind::Bound { target, .. } => self.is_constructor(&Value::Obj(target.clone())),
            ObjKind::Proxy(c) => match c.borrow().clone() {
                Some((t, _)) => self.is_constructor(&Value::Obj(t)),
                None => false,
            },
            _ => false,
        }
    }

    // ── Eigenschaften ────────────────────────────────────────────────────
    pub fn get(&mut self, base: &Value, key: &str) -> C<Value> {
        self.private_brand(base, key)?;
        // Primitive bekommen KEINE Huelle fuer einen blossen Lesezugriff —
        // ausser bei Zeichenketten, wo Laenge und Index direkt beantwortet
        // werden. Eine Huelle je Zugriff waere sonst der teuerste Weg zu
        // `s.length`.
        if let Value::Str(s) = base {
            if key == "length" { return Ok(Value::Num(s.chars().count() as f64)); }
            if let Some(i) = array_index(key) {
                return Ok(match s.chars().nth(i as usize) {
                    Some(c) => { let mut t = String::new(); t.push(c); Value::string(t) }
                    None => Value::Undefined,
                });
            }
        }
        let start = match base {
            Value::Obj(o) => o.clone(),
            Value::Undefined | Value::Null =>
                return self.type_err(&alloc::format!("cannot read '{key}' of {}",
                    if matches!(base, Value::Null) { "null" } else { "undefined" })),
            _ => self.to_object(base)?,
        };
        // **Eine SICHT beantwortet ihre Indizes selbst, und zwar ENDGUELTIG.**
        // Die Prototypenkette wird dabei NICHT gelaufen: `ta[99]` gibt
        // `undefined`, auch wenn `Object.prototype[99]` existiert. Das ist
        // kein Detail — es ist der Unterschied zwischen einer Sicht und einem
        // gewoehnlichen Objekt mit Zahlen als Schluesseln.
        if let Some(v) = ta_read(&start, key) { return Ok(v) }
        // Ein Stellvertreter beantwortet JEDEN Zugriff selbst — die
        // Prototypenkette darunter wird nicht gelaufen.
        if super::proxy::parts(&start).is_some() {
            return match super::proxy::trap(self, &start, "get")? {
                Some((f, t)) => {
                    let kv = super::proxy::key_value(key);
                    self.call(&f, Value::Undefined, &[t, kv, base.clone()])
                }
                None => { let t = super::proxy::target(self, &start)?; self.get(&Value::Obj(t), key) }
            };
        }
        // Array-`length` lebt in der Eigenschaftstabelle wie alles andere;
        // nur die Kette darunter wird hier gelaufen.
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(o) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = o.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(g) = &p.get {
                    if !matches!(g, Value::Undefined) {
                        return self.call(&g.clone(), base.clone(), &[]);
                    }
                }
                if p.is_accessor() { return Ok(Value::Undefined); }
                return Ok(p.value.clone().unwrap_or(Value::Undefined));
            }
            let next = o.borrow().proto.clone();
            cur = next;
        }
        Ok(Value::Undefined)
    }

    /// `Set(O, P, V, Throw)` (ES §7.3.4).
    ///
    /// **Die Wurf-Fahne ist ein ARGUMENT, kein Modus.** Das ist der Punkt, den
    /// beak bis 0.98.0 nicht hatte: nicht nur strenger Code will einen Fehler,
    /// wenn ein Schreiben scheitert, sondern auch fast jede eingebaute
    /// Funktion — `[].push` auf einem eingefrorenen Feld muss in BEIDEN Modi
    /// werfen. Deshalb steht die Fahne hier und wird an jeder Rufstelle
    /// entschieden; ein Vorgabewert haette genau die Haelfte still falsch
    /// gelassen.
    /// **Die Markenpruefung** (ES §7.3.28 `PrivateGet`/`PrivateSet`).
    ///
    /// Ein privates Feld ist keine Eigenschaft, die man anlegen kann: es
    /// entsteht im Konstruktor und nirgends sonst. Ein Zugriff auf ein
    /// Objekt, das es nicht hat, ist ein TypeError — nicht `undefined`, und
    /// erst recht kein stilles Anlegen. Ohne diese Pruefung war
    /// `fremdesObjekt.#f` schlicht `undefined`, und Schreiben legte das Feld
    /// an: die Kapselung war eine Verabredung, keine Grenze.
    /// `#x in obj` — die Markenpruefung als AUSDRUCK (ES §13.10.1).
    ///
    /// Sie WIRFT nicht, sie antwortet mit ja/nein; das ist ihr ganzer Zweck.
    /// Der Parser macht daraus einen Bezeichner `#x` — ein echter Name kann
    /// nie mit `#` anfangen, also ist das eindeutig.
    pub fn private_in(&mut self, name: &str, base: &Value) -> C<Value> {
        let Value::Obj(o) = base else {
            return self.type_err("the right side of 'in' must be an object");
        };
        let key = super::value::private_key(name);
        let o = o.clone();
        Ok(Value::Bool(self.has_property(&o, &key)))
    }

    fn private_brand(&mut self, base: &Value, key: &str) -> C<()> {
        if !key.starts_with(super::value::PRIVATE_PREFIX) { return Ok(()); }
        let ok = matches!(base, Value::Obj(o) if self.has_property(o, key));
        if ok { return Ok(()); }
        self.type_err(&alloc::format!(
            "cannot read private member #{} from an object whose class did not declare it",
            super::value::private_name(key)))
    }

    pub fn set(&mut self, base: &Value, key: &str, val: Value, throw: bool) -> C<()> {
        self.private_brand(base, key)?;
        let Value::Obj(o) = base else {
            // Zuweisung an eine Eigenschaft eines Primitivs verpufft still
            // (im lockeren Modus). Der strenge Modus wuerde werfen — das
            // gehoert zu den Dingen, die der Lauf als offen ausweist.
            if matches!(base, Value::Undefined | Value::Null) { strict_site!(self, 11); }
            else { strict_site!(self, 0); }
            if throw {
                // `undefined.x = 1` wirft ohnehin schon beim Lesen der Basis;
                // hier geht es um `"abc".x = 1` im strengen Modus.
                return self.type_err(&alloc::format!(
                    "cannot create property '{key}' on a primitive value"));
            }
            return Ok(());
        };
        // Dieselbe Endgueltigkeit beim Schreiben: ausserhalb der Sicht
        // verpufft es, und ein Setzer in der Kette bekommt es nie zu sehen.
        if let Some(t) = ta_of(o) {
            if let Some(k) = array_index(key) {
                // Die Umwandlung laeuft AUCH, wenn der Index draussen liegt —
                // sie ist beobachtbar.
                if t.kind.is_big() {
                    let big = self.to_bigint(&val)?;
                    let live = t.live_len();
                    if (k as usize) < live {
                        let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Ok(()) };
                        let at = t.offset + (k as usize) * t.kind.size();
                        t.kind.write_big(&mut b.bytes.borrow_mut(), at, &big);
                    }
                    return Ok(());
                }
                let n = self.to_number(&val)?;
                let live = t.live_len();
                if (k as usize) < live {
                    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Ok(()) };
                    let at = t.offset + (k as usize) * t.kind.size();
                    t.kind.write(&mut b.bytes.borrow_mut(), at, n);
                }
                return Ok(());
            }
        }
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "set")? {
                Some((f, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, Value::Undefined, &[t, kv, val, base.clone()])?;
                    if !r.truthy() {
                        strict_site!(self, 5);
                        if throw {
                            return self.type_err(&alloc::format!(
                                "'set' on proxy: trap returned falsish for property '{key}'"));
                        }
                    }
                    Ok(())
                }
                None => { let t = super::proxy::target(self, o)?; self.set(&Value::Obj(t), key, val, throw) }
            };
        }
        // Ein Setzer irgendwo in der Kette gewinnt vor dem eigenen Feld.
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = c.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(st) = &p.set {
                    if !matches!(st, Value::Undefined) {
                        self.call(&st.clone(), base.clone(), &[val])?;
                        return Ok(());
                    }
                }
                // Nur ein Getter, kein Setzer: das Schreiben scheitert.
                if p.is_accessor() {
                    strict_site!(self, 3);
                    if throw { return self.type_err(&alloc::format!(
                        "cannot set property '{key}' of #<Object> which has only a getter")); }
                    return Ok(());
                }
                if Rc::ptr_eq(&c, o) {
                    if !p.writable {
                        strict_site!(self, 1);
                        if throw { return self.type_err(&alloc::format!(
                            "cannot assign to read only property '{key}'")); }
                        return Ok(());
                    }
                    let mut np = p.clone();
                    np.value = Some(val);
                    o.borrow_mut().set_prop(Rc::from(key), np);
                    return Ok(());
                }
                if !p.writable {
                    strict_site!(self, 2);
                    if throw { return self.type_err(&alloc::format!(
                        "cannot assign to read only property '{key}'")); }
                    return Ok(());
                }
                break;
            }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        if !o.borrow().extensible {
            strict_site!(self, 4);
            if throw { return self.type_err(&alloc::format!(
                "cannot add property {key}, object is not extensible")); }
            return Ok(());
        }
        o.borrow_mut().set_prop(Rc::from(key), Prop::data(val));
        self.fix_array_length(o, key);
        Ok(())
    }

    /// Ein Array haelt `length` selbst nach: eine Zuweisung an einen Index
    /// jenseits der Laenge schiebt sie nach. Ohne das ist `push` gebaut, aber
    /// `a[0]=1; a.length` bleibt 0.
    fn fix_array_length(&mut self, o: &Gc, key: &str) {
        if !matches!(o.borrow().kind, ObjKind::Array) { return; }
        if let Some(i) = array_index(key) {
            let cur = o.borrow().get_own("length").and_then(|p| p.value.clone());
            let n = match cur { Some(Value::Num(n)) => n, _ => 0.0 };
            if (i as f64) >= n {
                o.borrow_mut().define("length", Prop {
                    value: Some(Value::Num(i as f64 + 1.0)), get: None, set: None,
                    writable: true, enumerable: false, configurable: false });
            }
        }
    }

    pub fn has_property(&mut self, o: &Gc, key: &str) -> bool {
        // Ein Stellvertreter kann hier WERFEN; `has_property` gibt aber nur
        // ein Ja/Nein. Der geworfene Wert geht dabei verloren — benannt statt
        // verschwiegen, `has_prop` daneben reicht ihn durch, und `in` ruft die.
        if super::proxy::parts(o).is_some() {
            return self.has_prop(o, key).unwrap_or(false);
        }
        if let Some(t) = ta_of(o) {
            if let Some(k) = array_index(key) {
                return (k as usize) < t.live_len();
            }
        }
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return false; }
            if c.borrow().has_own(key) { return true; }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        false
    }

    // ── Aufrufen ─────────────────────────────────────────────────────────
    pub fn call(&mut self, callee: &Value, this_val: Value, args: &[Value]) -> C<Value> {
        let Value::Obj(f) = callee else {
            return self.type_err("value is not a function");
        };
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return self.range_err("Maximum call stack size exceeded");
        }
        let r = self.call_inner(f, this_val, args);
        self.depth -= 1;
        r
    }

    fn call_inner(&mut self, f: &Gc, this_val: Value, args: &[Value]) -> C<Value> {
        // Die `apply`-Falle. Ohne sie liefe ein Aufruf auf einen
        // Stellvertreter am Behandler vorbei ans Ziel.
        if super::proxy::parts(f).is_some() {
            return match super::proxy::trap(self, f, "apply")? {
                Some((h, t)) => {
                    let arr = self.new_array(args.to_vec());
                    self.call(&h, Value::Undefined, &[t, this_val, arr])
                }
                None => { let t = super::proxy::target(self, f)?;
                          self.call(&Value::Obj(t), this_val, args) }
            };
        }
        enum Which { Native(Rc<NativeData>), Js(Rc<FuncData>), Bound(Gc, Value, Vec<Value>) }
        let which = match &f.borrow().kind {
            ObjKind::Native(n) => Which::Native(n.clone()),
            ObjKind::Function(d) => Which::Js(d.clone()),
            ObjKind::Bound { target, this_val, args } =>
                Which::Bound(target.clone(), this_val.clone(), args.clone()),
            _ => return self.type_err("value is not a function"),
        };
        match which {
            Which::Native(n) => (n.func)(self, this_val, args),
            Which::Bound(t, bt, mut ba) => {
                ba.extend_from_slice(args);
                self.call(&Value::Obj(t), bt, &ba)
            }
            Which::Js(d) => {
                // **Ein Generator laeuft seinen Rumpf hier NICHT.** Der Aufruf
                // baut ein Objekt, und der Rumpf faengt erst beim ersten
                // `next()` an — auf einer eigenen Maschine. Das ist die
                // einzige Stelle, an der ein Generatorobjekt entsteht; die
                // Befehlsmaschine schickt ihre Aufrufe absichtlich hierher.
                if d.node.is_generator && !d.node.is_async {
                    if let Some(v) = super::generator::make(self, f, &d, this_val.clone(), args)? {
                        return Ok(v);
                    }
                }
                // Und eine async-Funktion gibt ein VERSPRECHEN zurueck. Ihr
                // Rumpf laeuft bis zum ersten `await` sofort weiter, dann
                // haelt er an — dieselbe Maschine, nur wirft ihn die
                // Microtask-Schlange wieder an.
                if d.node.is_async && !d.node.is_generator {
                    if let Some(v) = super::generator::make_async(self, &d, this_val.clone(), args)? {
                        return Ok(v);
                    }
                }
                self.run_js_body(&d, this_val, args)
            }
        }
    }

    /// Die Umgebung, in der ein Aufruf laeuft — alles, was VOR dem ersten
    /// Schritt des Rumpfes passiert: `this`, `arguments`, das Heimatobjekt,
    /// die Parameter.
    ///
    /// Eigene Funktion, weil die Befehlsmaschine sie braucht: sie legt danach
    /// einen RAHMEN an, statt den Rumpf ueber den Rust-Stapel zu fahren. Zwei
    /// Umsetzungen dieses Vorspanns waeren zwei verschiedene Aufrufsemantiken,
    /// und das ist die teuerste Sorte Unterschied.
    pub fn call_env(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value])
        -> C<Rc<RefCell<Env>>> {
        let env = Env::new(Some(d.env.clone()), true);
        // Ein Pfeil bekommt KEIN eigenes `this` — dadurch findet `this_of`
        // das der umgebenden Funktion.
        env.borrow_mut().home = d.home_object.clone();
        // Die Strenge des RUMPFES, nicht die des Rufers: eine strenge
        // Funktion bleibt streng, egal wer sie ruft, und eine lockere bleibt
        // locker, auch wenn strenger Code sie aufruft. `Env::new` hat gerade
        // die des Definitionsortes geerbt; ein `"use strict"` im Rumpf legt
        // hier drauf.
        if d.node.strict { env.borrow_mut().strict = true; }
        if !d.node.is_arrow {
            let t = d.this_val.clone().unwrap_or(this_val);
            let t = self.bind_this(t, d.node.strict)?;
            env.borrow_mut().this_val = Some(t);
            let ao = self.make_arguments(args);
            env.borrow_mut().vars.insert(Rc::from("arguments"),
                Binding { value: ao, mutable: true, initialized: true });
        }
        self.bind_params(&d.node.params, args, &env)?;
        // **Instanzfelder einer BASISklasse stehen, bevor der Rumpf laeuft.**
        // Eine abgeleitete legt sie erst nach `super()` an: vorher hat sie
        // zwar schon ein Objekt, aber ein Initialisierer darf ein Feld der
        // Elternklasse sehen, und das gibt es erst danach.
        if let Some(c) = &d.class {
            if c.super_class.is_none() {
                let t = super::interp::env_this(&env);
                self.init_fields(d, &t)?;
            }
        }
        Ok(env)
    }

    /// `OrdinaryCallBindThis` (ES §10.2.1.2) — was `this` im Rumpf WIRKLICH ist.
    ///
    /// **Der Unterschied ist der Modus, und beide Seiten waren falsch.** Eine
    /// strenge Funktion bekommt den Wert unveraendert: `f()` sieht `undefined`.
    /// Eine lockere sieht dort `globalThis`, und ein Primitiv wird ihr
    /// EINGEPACKT — `(7).f()` sieht ein `Number`-Objekt, kein `7`. beak gab
    /// bis 0.98.0 immer den strengen Wert, in beiden Modi; das war der
    /// groesste einzelne Posten der Messung (257 Varianten, 191 davon locker).
    fn bind_this(&mut self, t: Value, strict: bool) -> C<Value> {
        if strict { return Ok(t); }
        match t {
            Value::Undefined | Value::Null => Ok(Value::Obj(self.realm.global.clone())),
            Value::Obj(_) => Ok(t),
            other => Ok(Value::Obj(self.to_object(&other)?)),
        }
    }

    /// Die Instanzfelder einer Klasse auf ein frisches `this` legen.
    ///
    /// Jeder Initialisierer ist ein eigener kleiner Funktionsbereich mit
    /// `this` auf der Instanz und `home` der Klasse — ein Pfeil darin faengt
    /// die INSTANZ ein, und `super.x` darin trifft die Elternklasse. Der
    /// Bereich darum ist der der KLASSE (`d.env`), nicht der des Aufrufers.
    pub fn init_fields(&mut self, d: &Rc<FuncData>, this_val: &Value) -> C<()> {
        let Some(c) = d.class.clone() else { return Ok(()) };
        let Value::Obj(o) = this_val else { return Ok(()) };
        for m in &c.body {
            let ClassMember::Field { key, value, is_static: false, .. } = m else { continue };
            let fenv = Env::new(Some(d.env.clone()), true);
            {
                let mut b = fenv.borrow_mut();
                b.this_val = Some(this_val.clone());
                b.home = d.home_object.clone();
            }
            let k = self.prop_key(key, &fenv)?;
            let v = match value {
                Some(e) => {
                    let val = self.eval(e, &fenv)?;
                    // `x = function(){}` gibt der Funktion den Feldnamen —
                    // dieselbe Regel wie bei `var f = function(){}`.
                    self.name_function(&val, &k);
                    val
                }
                None => Value::Undefined,
            };
            o.borrow_mut().set_prop(k, Prop::data(v));
        }
        Ok(())
    }

    /// Einen Funktionsrumpf mit dem BAUMLAEUFER fahren — der Weg, den ein
    /// Aufruf nimmt, wenn der Uebersetzer den Rumpf nicht kann.
    ///
    /// Eigene Funktion, weil `generator.rs` sie fuer denselben Fall braucht:
    /// eine async-Funktion mit unuebersetzbarem Rumpf muss trotzdem ein
    /// VERSPRECHEN zurueckgeben, und dafuer muss jemand den Rumpf fahren und
    /// den Ausgang einsammeln.
    pub fn run_js_body(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value]) -> C<Value> {
        let env = self.call_env(d, this_val, args)?;
        // Ein KONSTRUKTOR ohne `return` liefert sein `this` — und das kann
        // `super()` inzwischen umgehaengt haben. Bei einer Basisklasse ist es
        // dasselbe Objekt, das `construct` ohnehin nimmt; bei einer
        // abgeleiteten ist es der Unterschied.
        let implicit = || if d.class.is_some() { env_this(&env) } else { Value::Undefined };
        match self.run_body(&d.node.body, &env) {
            Ok(()) | Err(Abrupt::Return(Value::Undefined)) => Ok(implicit()),
            Err(Abrupt::Return(v)) => Ok(v),
            Err(e) => Err(e),
        }
    }

    /// Das Hochziehen eines Funktionsrumpfes — `var` und Funktionen nach vorn.
    /// Fuer die Maschine, die den Rumpf danach als Befehle faehrt.
    pub fn hoist_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        self.hoist(body, env, env)
    }

    /// Der uebersetzte Rumpf dieser Funktion, oder `None`, wenn der
    /// Uebersetzer absagt. Einmal je Funktion, dann gemerkt.
    pub fn func_chunk(&mut self, f: &Rc<Func>) -> Option<Rc<super::code::Chunk>> {
        if self.vm_off {
            return None;
        }
        let key = Rc::as_ptr(f) as usize;
        if let Some(c) = self.func_chunks.get(&key) {
            return c.clone();
        }
        let c = match super::compile::function(f) {
            Ok(ch) => Some(Rc::new(ch)),
            Err(u) => {
                *self.func_declines.entry(u.0).or_insert(0) += 1;
                None
            }
        };
        self.func_chunks.insert(key, c.clone());
        c
    }

    fn make_arguments(&mut self, args: &[Value]) -> Value {
        let g = new_kind(Some(self.realm.object_proto.clone()), ObjKind::Arguments);
        // `arguments` ist iterierbar — `[...arguments]` und `for (a of
        // arguments)` sind gewoehnliche Schreibweisen, und beide gehen ueber
        // dieselbe Funktion wie `Array.prototype.values`.
        {
            let vals = self.get(&Value::Obj(self.realm.array_proto.clone()), "values");
            if let Ok(v) = vals { g.borrow_mut().define(SYM_ITERATOR, Prop::builtin(v)); }
        }
        {
            let mut o = g.borrow_mut();
            for (i, a) in args.iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(a.clone()));
            }
            o.define("length", Prop::builtin(Value::Num(args.len() as f64)));
        }
        Value::Obj(g)
    }

    fn bind_params(&mut self, params: &[Pat], args: &[Value], env: &Rc<RefCell<Env>>) -> C<()> {
        // **Erst ALLE Parameternamen HIER anlegen, dann binden.**
        //
        // `bind_pattern(…, true)` bindet ueber `init_binding`, und das laeuft
        // die Umgebungskette HOCH: ein Parameter, der so heisst wie eine
        // Variable weiter aussen, schrieb in DIESE. Minifizierter Code
        // benutzt ueberall dieselben kurzen Namen, also traf das jedes
        // Bundle — auf der Fritzbox-Anmeldeseite hat `o=(o,a)=>{…}` beim
        // ersten Aufruf das aeussere `o` mit einer 0 ueberschrieben, und der
        // naechste Zugriff darauf war „bind is not a function".
        //
        // Vor dem Binden, nicht je Parameter: `function f(a = b, b)` ist ein
        // ReferenceError und darf nicht das aeussere `b` finden
        // (`FunctionDeclarationInstantiation`, ES §10.2.11 Schritt 21).
        for p in params {
            let mut names = Vec::new();
            super::eval::names_of(p, &mut names);
            for n in names {
                env.borrow_mut().vars.insert(Rc::from(n.as_str()),
                    Binding { value: Value::Undefined, mutable: true, initialized: false });
            }
        }
        let mut i = 0;
        for p in params {
            if let Pat::Rest(inner) = p {
                let rest: Vec<Value> = args.iter().skip(i).cloned().collect();
                let arr = self.new_array(rest);
                self.bind_pattern(inner, arr, env, true)?;
                break;
            }
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            self.bind_pattern(p, v, env, true)?;
            i += 1;
        }
        Ok(())
    }

    // ── Programm ─────────────────────────────────────────────────────────
    pub fn run_program(&mut self, prog: &Program) -> C<Value> {
        let env = self.realm.global_env.clone();
        // Die globale Umgebung gehoert der EINHEIT, die gerade laeuft: ein
        // Skript mit `"use strict"` faerbt sie streng, das naechste ohne
        // wieder locker. Beide Faelle kommen im test262-Lauf vor — Vorspann
        // locker, Testkoerper streng.
        env.borrow_mut().strict = prog.strict;
        // Hochziehen ist fuer BEIDE Maschinen dasselbe: es arbeitet auf der
        // Umgebung, nicht auf dem Code.
        self.hoist(&prog.body, &env, &env)?;
        // **Ganz oder gar nicht.** Was der Uebersetzer kann, faehrt die
        // Befehlsmaschine; sagt er irgendwo nein, faehrt der Baumlaeufer das
        // GANZE Programm. Eine Mischung waere ein zweiter Semantikpfad im
        // selben Lauf, und solche laufen still auseinander.
        let r = match if self.vm_off { Err(super::code::Unsupported("off")) }
                      else { super::compile::program(prog) } {
            Ok(chunk) => {
                self.vm_ran += 1;
                self.vm_decline = None;
                let mut vm = super::vm::Vm::new();
                vm.run(self, Rc::new(chunk), &env)
            }
            Err(u) => {
                self.vm_declined += 1;
                self.vm_decline = Some(u.0);
                (|| -> C<Value> {
                    let mut last = Value::Undefined;
                    for st in &prog.body {
                        if let Some(v) = self.exec(st, &env)? { last = v; }
                    }
                    Ok(last)
                })()
            }
        };
        // Auch wenn das Programm geworfen hat: die Schlange gehoert geleert.
        // Ein `.then`, das vor dem Fehler angelegt wurde, ist angemeldet.
        super::promise::run_jobs(self);
        r
    }

    /// `eval`. Der Unterschied zwischen DIREKT und indirekt ist der Bereich:
    /// `eval(s)` als blosser Aufruf laeuft im Bereich des Rufers und sieht
    /// dessen Namen, `(0,eval)(s)` laeuft global.
    ///
    /// **Kein strenger Modus.** Die Engine kennt ihn zur Laufzeit nicht
    /// (siehe [[project-beak-js-language-gap]]), also legt auch ein
    /// `"use strict"` im Quelltext keinen eigenen Bereich fuer `var` an.
    pub fn perform_eval(&mut self, code: &Value, caller: Option<Rc<RefCell<Env>>>) -> C<Value> {
        // Alles, was keine Zeichenkette ist, kommt unveraendert zurueck —
        // `eval(42)` ist 42, kein Programm.
        let Value::Str(src) = code else { return Ok(code.clone()) };
        // Ein `eval` legt einen RUST-Rahmen an (Parser + eigene Maschine) und
        // laeuft sonst ohne Grenze im Kreis: `eval("eval('…')")`. Also zaehlt
        // er wie ein Aufruf mit.
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(self.throw_kind("RangeError", "maximum call stack size exceeded"));
        }
        let r = self.eval_inner(src, caller);
        self.depth -= 1;
        r
    }

    fn eval_inner(&mut self, src: &Rc<str>, caller: Option<Rc<RefCell<Env>>>) -> C<Value> {
        let prog = match super::parser::parse(src, false) {
            Ok(p) => p,
            Err(e) => return Err(self.throw_kind("SyntaxError", &e.msg)),
        };
        // Ein DIREKTES eval erbt die Strenge seines Rufers; die eigene
        // Direktive legt drauf. Ein indirektes faengt locker an.
        let inherited = caller.as_ref().is_some_and(|e| e.borrow().strict);
        let strict = prog.strict || inherited;
        let base = caller.unwrap_or_else(|| self.realm.global_env.clone());
        // `let`/`const` bekommen einen EIGENEN Bereich, `var` und
        // Funktionsdeklarationen steigen bis zur naechsten Funktionsgrenze
        // des RUFERS — genau deshalb ist der Bereich hier kein Funktionsbereich.
        let scope = Env::new(Some(base.clone()), false);
        scope.borrow_mut().strict = strict;
        // **Strenger eval behaelt sein `var` bei sich.** Sonst steigt es bis
        // zur Funktionsgrenze des Rufers und legt dort einen Namen an, den
        // der Rufer nie geschrieben hat — genau der Unterschied, an dem
        // `eval` in 0.92.0 fuenf Tests gekostet hat.
        let var_env = if strict {
            scope.borrow_mut().is_func_scope = true;
            scope.clone()
        } else {
            let mut ve = base;
            loop {
                let is_fn = ve.borrow().is_func_scope;
                if is_fn { break }
                let up = ve.borrow().parent.clone();
                match up { Some(p) => ve = p, None => break }
            }
            ve
        };
        self.hoist(&prog.body, &scope, &var_env)?;
        // Dieselbe Wahl wie beim Programm: kann der Uebersetzer alles, faehrt
        // die Maschine; sonst der Baumlaeufer. Nie eine Mischung.
        match if self.vm_off { Err(super::code::Unsupported("off")) }
              else { super::compile::program(&prog) } {
            Ok(chunk) => {
                self.vm_ran += 1;
                let mut vm = super::vm::Vm::new();
                vm.run(self, Rc::new(chunk), &scope)
            }
            Err(u) => {
                self.vm_declined += 1;
                self.vm_decline = Some(u.0);
                let mut last = Value::Undefined;
                for st in &prog.body {
                    if let Some(v) = self.exec(st, &scope)? { last = v; }
                }
                Ok(last)
            }
        }
    }

    /// Ist dieser Wert die eingebaute `eval`? Nur dann ist ein Aufruf
    /// `eval(...)` ein DIREKTER — jede andere Funktion unter dem Namen ist
    /// ein gewoehnlicher Aufruf.
    pub fn is_eval_fn(&self, v: &Value) -> bool {
        let Some(want) = self.realm.eval_fn.as_ref() else { return false };
        matches!(v, Value::Obj(o) if Rc::ptr_eq(o, want))
    }

    fn run_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        self.hoist(body, env, env)?;
        for st in body { self.exec(st, env)?; }
        Ok(())
    }

    /// `var` und Funktionsdeklarationen nach vorn ziehen.
    ///
    /// `var` steigt bis zur naechsten FUNKTIONSGRENZE, `let`/`const`/`class`
    /// bleiben im Block und stehen bis zur Deklaration auf „nicht bereit" —
    /// das ist die zeitliche Totzone, und ohne sie ist `let` nur ein `var`
    /// mit anderem Namen.
    fn hoist(&mut self, body: &[Stmt], block: &Rc<RefCell<Env>>, func: &Rc<RefCell<Env>>) -> C<()> {
        for st in body { self.hoist_vars(st, func); }
        for st in body {
            // `export function f(){}` ist eine Deklaration mit einem Wort
            // davor — sie wird genauso hochgezogen. Ohne diese Zeile stuende
            // eine exportierte Funktion erst da, wenn ihre Zeile lief, und
            // ein Zyklus im Modulgraphen saehe sie nie.
            let st = super::modules::unexport(st).unwrap_or(st);
            match st {
                Stmt::Func(f) => {
                    if let Some(n) = &f.name {
                        let v = self.make_closure(f.clone(), block, None);
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: v, mutable: true, initialized: true });
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    let mut names = Vec::new();
                    for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                    for n in names {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()), Binding {
                            value: Value::Undefined,
                            mutable: d.kind != VarKind::Const,
                            initialized: false,
                        });
                    }
                }
                // `export default` legt seinen Wert unter einem Namen ab,
                // den kein Skript schreiben kann. Er wird HIER angelegt,
                // damit ein Zyklus, der ihn zu frueh liest, „nicht bereit"
                // sagt statt „gibt es nicht" — und damit eine
                // Funktionsdeklaration auch dahinter hochgezogen wird.
                Stmt::ExportDefault(d) => {
                    let (v, init, own) = match &**d {
                        ExportDefault::Func(f) =>
                            (self.make_closure(f.clone(), block, None), true, f.name.clone()),
                        ExportDefault::Class(c) => (Value::Undefined, false, c.name.clone()),
                        ExportDefault::Expr(_) => (Value::Undefined, false, None),
                    };
                    if let Some(n) = own {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: v.clone(), mutable: true, initialized: init });
                    }
                    block.borrow_mut().vars.insert(Rc::from(super::modules::DEFAULT_LOCAL),
                        Binding { value: v, mutable: true, initialized: init });
                }
                Stmt::Class(c) => {
                    if let Some(n) = &c.name {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: Value::Undefined, mutable: true, initialized: false });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `var` durch Bloecke und Schleifen hindurch einsammeln — aber NICHT
    /// durch Funktionen: dort faengt ein neuer Bereich an.
    fn hoist_vars(&mut self, st: &Stmt, func: &Rc<RefCell<Env>>) {
        let st = super::modules::unexport(st).unwrap_or(st);
        let mut put = |names: Vec<String>| {
            for n in names {
                let key: Rc<str> = Rc::from(n.as_str());
                if !func.borrow().vars.contains_key(&key) {
                    func.borrow_mut().vars.insert(key,
                        Binding { value: Value::Undefined, mutable: true, initialized: true });
                }
            }
        };
        match st {
            Stmt::VarDecl(d) if d.kind == VarKind::Var => {
                let mut names = Vec::new();
                for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                put(names);
            }
            Stmt::Block(b) => for s in b { self.hoist_vars(s, func) },
            Stmt::If { cons, alt, .. } => {
                self.hoist_vars(cons, func);
                if let Some(a) = alt { self.hoist_vars(a, func); }
            }
            Stmt::For { init, body, .. } => {
                if let Some(i) = init {
                    if let ForInit::VarDecl(d) = &**i {
                        if d.kind == VarKind::Var {
                            let mut names = Vec::new();
                            for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                            put(names);
                        }
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
                if let ForHead::VarDecl(d) = &**left {
                    if d.kind == VarKind::Var {
                        let mut names = Vec::new();
                        for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                        put(names);
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. }
            | Stmt::Labeled { body, .. } | Stmt::With { body, .. } => self.hoist_vars(body, func),
            Stmt::Try { block, handler, finalizer } => {
                for s in block { self.hoist_vars(s, func); }
                if let Some(h) = handler { for s in &h.body { self.hoist_vars(s, func); } }
                if let Some(f) = finalizer { for s in f { self.hoist_vars(s, func); } }
            }
            Stmt::Switch { cases, .. } => {
                for c in cases { for s in &c.body { self.hoist_vars(s, func); } }
            }
            _ => {}
        }
    }

    // ── Der Iteratorvertrag ──────────────────────────────────────────────

    /// `{ value, done }` — das Ergebnis eines `next()`.
    /// Ein Eigenschaftsbeschreiber (`{value, writable, …}`) → `Prop`.
    ///
    /// Eigene Funktion, weil sie fuenf Rufer hat: `Object.defineProperty`,
    /// `Object.defineProperties`, `Object.create` mit zweitem Argument und
    /// `Reflect.defineProperty`. Fuenf Kopien dieser Regeln waeren fuenf
    /// Gelegenheiten, sie auseinanderlaufen zu lassen.
    /// `ToPropertyDescriptor` (ES §6.2.6.5).
    ///
    /// **Gefragt wird nach ANWESENHEIT, nicht nach Wahrheit.** Die alte
    /// Fassung las `writable` mit `.truthy()`, und ein fehlendes Feld wurde
    /// damit zu `false` — beim ANLEGEN richtig, beim AENDERN falsch. Der
    /// Unterschied ist der ganze Sinn eines partiellen Beschreibers.
    ///
    /// Gefragt wird ueber die PROTOTYPKETTE (`HasProperty`, nicht `has_own`):
    /// ein Beschreiber, der `writable` erbt, zaehlt.
    pub fn to_prop_desc(&mut self, d: &Value) -> C<Desc> {
        let Value::Obj(dd) = d else {
            return self.type_err("property descriptor must be an object");
        };
        let dd = dd.clone();
        let mut out = Desc::default();
        if self.has_property(&dd, "enumerable") {
            out.enumerable = Some(self.get(d, "enumerable")?.truthy());
        }
        if self.has_property(&dd, "configurable") {
            out.configurable = Some(self.get(d, "configurable")?.truthy());
        }
        if self.has_property(&dd, "value") { out.value = Some(self.get(d, "value")?); }
        if self.has_property(&dd, "writable") {
            out.writable = Some(self.get(d, "writable")?.truthy());
        }
        if self.has_property(&dd, "get") {
            let g = self.get(d, "get")?;
            if !self.is_callable(&g) && !matches!(g, Value::Undefined) {
                return self.type_err("getter must be a function");
            }
            out.get = Some(g);
        }
        if self.has_property(&dd, "set") {
            let st = self.get(d, "set")?;
            if !self.is_callable(&st) && !matches!(st, Value::Undefined) {
                return self.type_err("setter must be a function");
            }
            out.set = Some(st);
        }
        if out.is_accessor() && out.is_data() {
            return self.type_err(
                "property descriptor cannot be both an accessor and a data descriptor");
        }
        Ok(out)
    }

    /// Und zurueck: `Prop` → Beschreiberobjekt.
    pub fn from_prop_desc(&mut self, p: &Prop) -> Value {
        let d = new_obj(Some(self.realm.object_proto.clone()));
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
        Value::Obj(d)
    }

    /// Die Eigenschaften aus einem `{k: beschreiber, …}` auf ein Objekt
    /// legen — `Object.defineProperties` und `Object.create(p, props)`.
    pub fn define_props_from(&mut self, target: &Gc, props: &Value) -> C<()> {
        let Value::Obj(src) = props else {
            return self.type_err("properties must be an object");
        };
        let keys: Vec<Rc<str>> = src.borrow().own_keys().into_iter()
            .filter(|k| src.borrow().is_enumerable(k))
            .collect();
        // ERST alle Beschreiber lesen, DANN alle anwenden (ES §20.1.2.3.1).
        // Die Reihenfolge ist beobachtbar: wirft der dritte Beschreiber,
        // duerfen die ersten beiden nicht schon gelegt sein.
        let mut pending = Vec::new();
        for k in keys {
            let d = self.get(props, &k)?;
            pending.push((k, self.to_prop_desc(&d)?));
        }
        for (k, d) in pending {
            self.define_or_throw(target, &k, d)?;
        }
        Ok(())
    }

    /// Ein frischer `ArrayBuffer` mit `n` Nullbytes.
    ///
    /// **Ueber `MAX_BUFFER_BYTES` gibt es einen leeren Puffer** — die Rufer
    /// pruefen vorher und werfen einen `RangeError`, so wie ein echter Motor
    /// es tut, wenn die Zuteilung scheitert. Ohne die Schranke hat ein
    /// test262-Fall 7 Petabyte angefordert und den Lauf mit SIGABRT beendet;
    /// in einem Kernel waere das kein Absturz des Testlaeufers, sondern der
    /// des Systems.
    pub fn new_buffer(&mut self, n: usize) -> Value {
        if n > MAX_BUFFER_BYTES { return self.new_buffer(0) }
        Value::Obj(new_kind(Some(self.realm.buffer_proto.clone()),
            ObjKind::Buffer(Rc::new(BufData {
                bytes: RefCell::new(alloc::vec![0u8; n]),
                detached: core::cell::Cell::new(false),
            }))))
    }

    /// Eine SICHT auf einen bestehenden Puffer — kein neuer Speicher.
    pub fn new_view(&mut self, kind: ElemKind, buf: Gc, offset: usize, len: usize) -> Value {
        let proto = self.realm.ta_protos.get(kind.name()).cloned()
            .unwrap_or_else(|| self.realm.typed_proto.clone());
        Value::Obj(new_kind(Some(proto),
            ObjKind::TypedArray(Rc::new(TaData { buf, kind, offset, len }))))
    }

    /// Eine Sicht MIT eigenem Puffer — der gewoehnliche `new Uint8Array(n)`.
    pub fn new_typed(&mut self, kind: ElemKind, len: usize) -> Value {
        let Some(bytes) = len.checked_mul(kind.size()) else { return Value::Undefined };
        let b = self.new_buffer(bytes);
        let Value::Obj(bo) = b else { return Value::Undefined };
        self.new_view(kind, bo, 0, len)
    }

    pub fn iter_result(&mut self, value: Value, done: bool) -> Value {
        let g = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut o = g.borrow_mut();
            o.define("value", Prop::data(value));
            o.define("done", Prop::data(Value::Bool(done)));
        }
        Value::Obj(g)
    }

    /// Ein Array-Iterator ueber `target`. `kind`: 0 Werte, 1 Schluessel,
    /// 2 Paare.
    pub fn array_iter(&mut self, target: Value, kind: u8) -> C<Value> {
        // `ToObject` zuerst: `Array.prototype.values.call("ab")` muss gehen.
        let t = self.to_object(&target)?;
        let g = new_obj(Some(self.realm.array_iter_proto.clone()));
        {
            let mut o = g.borrow_mut();
            o.define(IT_TARGET, Prop::data(Value::Obj(t)));
            o.define(IT_INDEX, Prop::data(Value::Num(0.0)));
            o.define(IT_KIND, Prop::frozen(Value::Num(kind as f64)));
        }
        Ok(Value::Obj(g))
    }

    /// `GetIterator`. Wirft, wenn der Wert keinen `Symbol.iterator` hat —
    /// genau das verlangt `for..of`, und der Fehlertext nennt den Grund.
    pub fn get_iterator(&mut self, v: &Value) -> C<Value> {
        if matches!(v, Value::Undefined | Value::Null) {
            return self.type_err("value is not iterable");
        }
        let m = self.get(v, SYM_ITERATOR)?;
        if !self.is_callable(&m) { return self.type_err("value is not iterable"); }
        let it = self.call(&m, v.clone(), &[])?;
        if !matches!(it, Value::Obj(_)) {
            return self.type_err("Symbol.iterator did not return an object");
        }
        Ok(it)
    }

    /// Wie `has_property`, aber ein Wurf aus einer Stellvertreterfalle kommt
    /// durch. `in` und `Reflect.has` rufen diese.
    pub fn has_prop(&mut self, o: &Gc, key: &str) -> C<bool> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "has")? {
                Some((f, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, Value::Undefined, &[t, kv])?;
                    Ok(r.truthy())
                }
                None => { let t = super::proxy::target(self, o)?; self.has_prop(&t, key) }
            };
        }
        Ok(self.has_property(o, key))
    }

    /// Die EIGENEN Schluessel — durch einen Stellvertreter hindurch.
    pub fn own_keys_of(&mut self, o: &Gc) -> C<Vec<PropName>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "ownKeys")? {
                Some((f, t)) => {
                    let r = self.call(&f, Value::Undefined, &[t])?;
                    let items = self.elems(&r)?;
                    let mut out = Vec::with_capacity(items.len());
                    for v in items { out.push(PropName::from(&*self.to_prop_key(&v)?)); }
                    Ok(out)
                }
                None => { let t = super::proxy::target(self, o)?; self.own_keys_of(&t) }
            };
        }
        Ok(o.borrow().own_keys())
    }

    /// Der EIGENE Beschreiber — durch einen Stellvertreter hindurch.
    pub fn get_own_desc(&mut self, o: &Gc, key: &str) -> C<Option<Prop>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "getOwnPropertyDescriptor")? {
                Some((f, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, Value::Undefined, &[t, kv])?;
                    if matches!(r, Value::Undefined) { return Ok(None); }
                    if !matches!(r, Value::Obj(_)) {
                        return self.type_err("getOwnPropertyDescriptor trap did not return an object");
                    }
                    // Die Falle liefert einen partiellen Beschreiber; eine
                    // ABGELEGTE Eigenschaft ist immer vollstaendig, also
                    // bekommen die fehlenden Felder hier ihre Vorgabewerte.
                    Ok(Some(self.to_prop_desc(&r)?.into_new_prop()))
                }
                None => { let t = super::proxy::target(self, o)?; self.get_own_desc(&t, key) }
            };
        }
        Ok(o.borrow().get_own(key).cloned())
    }

    /// Eine Eigenschaft festlegen — durch einen Stellvertreter hindurch.
    /// `[[DefineOwnProperty]]` — MIT der Pruefung (ES §10.1.6).
    ///
    /// Bis 0.99.0 legte diese Funktion einfach ab, was man ihr gab. Eine
    /// nicht konfigurierbare Eigenschaft liess sich damit umdefinieren,
    /// `Object.freeze` war eine Bitte, und `Object.defineProperty` gab immer
    /// `true`. 368 test262-Varianten haengen daran.
    pub fn define_own(&mut self, o: &Gc, key: &str, d: Desc) -> C<bool> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "defineProperty")? {
                Some((f, t)) => {
                    let kv = super::proxy::key_value(key);
                    let dv = self.desc_to_object(&d);
                    let r = self.call(&f, Value::Undefined, &[t, kv, dv])?;
                    Ok(r.truthy())
                }
                None => { let t = super::proxy::target(self, o)?; self.define_own(&t, key, d) }
            };
        }
        let cur = o.borrow().get_own(key).cloned();
        let extensible = o.borrow().extensible;
        let Some(cur) = cur else {
            // Neu. Nur die Erweiterbarkeit steht dem im Weg; die fehlenden
            // Felder bekommen HIER ihre Vorgabewerte.
            if !extensible { return Ok(false); }
            let np = d.into_new_prop();
            o.borrow_mut().define(key, np);
            self.fix_array_length(o, key);
            return Ok(true);
        };
        if d.is_empty() { return Ok(true); }
        // `ValidateAndApplyPropertyDescriptor`, Schritt 4: was eine nicht
        // konfigurierbare Eigenschaft NICHT erlaubt.
        if !cur.configurable {
            if d.configurable == Some(true) { return Ok(false); }
            if let Some(e) = d.enumerable { if e != cur.enumerable { return Ok(false); } }
            if !d.is_generic() && d.is_accessor() != cur.is_accessor() { return Ok(false); }
            if cur.is_accessor() {
                let same = |a: &Option<Value>, b: &Option<Value>| {
                    let av = a.clone().unwrap_or(Value::Undefined);
                    let bv = b.clone().unwrap_or(Value::Undefined);
                    av.same_value(&bv)
                };
                if d.get.is_some() && !same(&d.get, &cur.get) { return Ok(false); }
                if d.set.is_some() && !same(&d.set, &cur.set) { return Ok(false); }
            } else if !cur.writable {
                if d.writable == Some(true) { return Ok(false); }
                if let Some(v) = &d.value {
                    let cv = cur.value.clone().unwrap_or(Value::Undefined);
                    if !v.same_value(&cv) { return Ok(false); }
                }
            }
        }
        // Angewendet wird FELDWEISE: was der Beschreiber nicht nennt, bleibt.
        let mut np = cur.clone();
        if !d.is_generic() && d.is_accessor() != cur.is_accessor() {
            // Art gewechselt — die Felder der alten Art fallen auf ihre
            // Vorgabewerte zurueck, `enumerable`/`configurable` bleiben.
            np = Prop { value: None, get: None, set: None, writable: false,
                        enumerable: cur.enumerable, configurable: cur.configurable };
        }
        if let Some(v) = d.value { np.value = Some(v); np.get = None; np.set = None; }
        if let Some(w) = d.writable { np.writable = w; }
        if let Some(g) = d.get { np.get = Some(g); np.value = None; }
        if let Some(st) = d.set { np.set = Some(st); np.value = None; }
        if let Some(e) = d.enumerable { np.enumerable = e; }
        if let Some(c) = d.configurable { np.configurable = c; }
        o.borrow_mut().define(key, np);
        self.fix_array_length(o, key);
        Ok(true)
    }

    /// `FromPropertyDescriptor` fuer einen PARTIELLEN Beschreiber — was die
    /// Stellvertreter-Falle zu sehen bekommt. Nur die Felder, die dastehen.
    pub fn desc_to_object(&mut self, d: &Desc) -> Value {
        let o = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut b = o.borrow_mut();
            if let Some(v) = &d.value { b.define("value", Prop::data(v.clone())); }
            if let Some(w) = d.writable { b.define("writable", Prop::data(Value::Bool(w))); }
            if let Some(g) = &d.get { b.define("get", Prop::data(g.clone())); }
            if let Some(s) = &d.set { b.define("set", Prop::data(s.clone())); }
            if let Some(e) = d.enumerable { b.define("enumerable", Prop::data(Value::Bool(e))); }
            if let Some(c) = d.configurable { b.define("configurable", Prop::data(Value::Bool(c))); }
        }
        Value::Obj(o)
    }

    /// `DefinePropertyOrThrow` (ES §7.3.8) — was die Eingebauten benutzen.
    pub fn define_or_throw(&mut self, o: &Gc, key: &str, d: Desc) -> C<()> {
        if self.define_own(o, key, d)? { return Ok(()); }
        self.type_err(&alloc::format!("cannot redefine property: {key}"))
    }

    /// Der Prototyp — durch einen Stellvertreter hindurch.
    pub fn proto_of(&mut self, o: &Gc) -> C<Option<Gc>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "getPrototypeOf")? {
                Some((f, t)) => {
                    let r = self.call(&f, Value::Undefined, &[t])?;
                    Ok(match r { Value::Obj(x) => Some(x), _ => None })
                }
                None => { let t = super::proxy::target(self, o)?; self.proto_of(&t) }
            };
        }
        Ok(o.borrow().proto.clone())
    }

    /// Ein Schritt. `None` heisst fertig.
    pub fn iter_next(&mut self, it: &Value) -> C<Option<Value>> {
        self.tick()?;
        let f = self.get(it, "next")?;
        if !self.is_callable(&f) { return self.type_err("iterator has no next method"); }
        let r = self.call(&f, it.clone(), &[])?;
        if !matches!(r, Value::Obj(_)) {
            return self.type_err("iterator result is not an object");
        }
        let done = self.get(&r, "done")?;
        if done.truthy() { return Ok(None); }
        Ok(Some(self.get(&r, "value")?))
    }

    /// `IteratorClose` — beim vorzeitigen Verlassen (`break`, `return`, ein
    /// Fehler im Rumpf). Ein Generator raeumt hier auf; wer das auslaesst,
    /// laesst `finally`-Bloecke in fremdem Code liegen.
    ///
    /// Ein Fehler AUS `return()` wird geschluckt: der Grund fuers Verlassen
    /// steht schon fest, und ihn zu ueberschreiben verbirgt ihn.
    pub fn iter_close(&mut self, it: &Value) {
        let Ok(f) = self.get(it, "return") else { return };
        if !self.is_callable(&f) { return; }
        let _ = self.call(&f, it.clone(), &[]);
    }

    /// Alles auf einmal — fuer Streuung, `Array.from`, `new Map(…)`.
    ///
    /// Eifrig, und das ist hier richtig: alle drei Aufrufer BRAUCHEN die
    /// vollstaendige Liste. `for..of` laeuft nicht hier durch, sondern
    /// schrittweise ([[exec_for_of]]) — sonst haenge ein unendlicher
    /// Iterator die Seite auf, obwohl der Rumpf im ersten Durchlauf
    /// abbricht.
    pub fn iterate(&mut self, v: &Value) -> C<Vec<Value>> {
        let it = self.get_iterator(v)?;
        let mut out = Vec::new();
        loop {
            match self.iter_next(&it) {
                Ok(Some(x)) => out.push(x),
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Die Schluessel, ueber die ein `for..in` laeuft: aufzaehlbar, die ganze
    /// Prototypenkette hoch, ohne Doppelte.
    ///
    /// **Die Liste wird VORHER gebaut.** Eine Aenderung am Objekt waehrend der
    /// Schleife darf sie nicht ins Rutschen bringen — das ist der Unterschied
    /// zu `for..of`, das faul sein MUSS. Ein Schluessel, der zwischendurch
    /// verschwindet, wird trotzdem uebersprungen: das macht `get` von selbst.
    ///
    /// `undefined`/`null` geben eine LEERE Liste, keinen Fehler: `for (k in
    /// null)` laeuft null Mal, statt zu werfen.
    ///
    /// Eigene Funktion, weil die Befehlsmaschine sie braucht. Sie dort
    /// nachzubauen waere eine zweite Aufzaehlungsreihenfolge, und die faellt
    /// erst auf, wenn ein Skript sich darauf verlaesst.
    pub fn for_in_keys(&mut self, v: &Value) -> C<Vec<Rc<str>>> {
        if matches!(v, Value::Undefined | Value::Null) { return Ok(Vec::new()) }
        let o = self.to_object(v)?;
        let mut keys: Vec<Rc<str>> = Vec::new();
        let mut cur = Some(o);
        let mut hops = 0;
        while let Some(c) = cur {
            if hops > MAX_PROTO_CHAIN { break }
            hops += 1;
            let own = self.own_keys_of(&c)?;
            for k in own {
                if is_sym_key(&k) { continue }
                let enumerable = if super::proxy::parts(&c).is_some() {
                    matches!(self.get_own_desc(&c, &k)?, Some(p) if p.enumerable)
                } else { c.borrow().is_enumerable(&k) };
                if enumerable && !keys.iter().any(|x| *x == k) { keys.push(k); }
            }
            let next = self.proto_of(&c)?;
            cur = next;
        }
        Ok(keys)
    }

    /// `CreateListFromArrayLike`: `length` und Indizes, OHNE den
    /// Iteratorvertrag.
    ///
    /// Das ist kein Rueckfall, sondern eine eigene Spezifikationsoperation.
    /// `Function.prototype.apply` und die Array-Methoden benutzen sie —
    /// `apply` mit einem Objekt ohne `Symbol.iterator` muss gehen, `for..of`
    /// damit muss werfen. Wer beides in eine Funktion legt, verliert genau
    /// diesen Unterschied.
    pub fn elems(&mut self, v: &Value) -> C<Vec<Value>> {
        match v {
            Value::Str(s) => Ok(s.chars().map(|c| {
                let mut t = String::new(); t.push(c); Value::string(t)
            }).collect()),
            Value::Obj(_) => {
                let len = self.get(v, "length")?;
                let n = self.to_number(&len)?;
                let n = if n.is_finite() && n > 0.0 { n as usize } else { 0 };
                let mut out = Vec::with_capacity(n.min(1 << 16));
                for i in 0..n {
                    self.tick()?;
                    out.push(self.get(v, &num_to_string(i as f64))?);
                }
                Ok(out)
            }
            _ => self.type_err("value is not array-like"),
        }
    }

    pub fn new_array(&mut self, items: Vec<Value>) -> Value {
        let g = new_kind(Some(self.realm.array_proto.clone()), ObjKind::Array);
        {
            let mut o = g.borrow_mut();
            let n = items.len();
            for (i, v) in items.into_iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(v));
            }
            o.define("length", Prop {
                value: Some(Value::Num(n as f64)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }

    pub fn make_closure(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>) -> Value {
        self.make_method(f, env, this_val, None)
    }

    /// Dasselbe, aber mit Heimatobjekt — das ist der einzige Unterschied
    /// zwischen einer Funktion und einer Methode, und `super` haengt daran.
    pub fn make_method(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>,
                       home: Option<Gc>) -> Value {
        // Eine Generatorfunktion haengt unter `%GeneratorFunction.prototype%`,
        // nicht unter `Function.prototype` — daran haengen `f.constructor` und
        // der `toStringTag`, und beides wird gemessen.
        let fproto = if f.is_generator && !f.is_async {
            self.realm.generator_func_proto.clone()
        } else {
            self.realm.function_proto.clone()
        };
        let g = new_kind(Some(fproto),
            ObjKind::Function(Rc::new(FuncData {
                node: f.clone(), env: env.clone(), this_val, home_object: home,
                class: None,
            })));
        {
            let mut o = g.borrow_mut();
            let len = f.params.iter().take_while(|p| matches!(p, Pat::Ident(_))).count();
            o.define("length", Prop { value: Some(Value::Num(len as f64)), get: None, set: None,
                writable: false, enumerable: false, configurable: true });
            o.define("name", Prop { value: Some(Value::str(f.name.as_deref().unwrap_or(""))),
                get: None, set: None, writable: false, enumerable: false, configurable: true });
        }
        // Ein Pfeil hat kein `prototype` — er kann nicht als Konstruktor
        // dienen, und ein vorhandenes `prototype` waere ein sichtbarer
        // Unterschied zu jedem echten Motor.
        // Eine async-Funktion hat KEIN `prototype` — sie ist kein Konstruktor,
        // und ein vorhandenes waere ein sichtbarer Unterschied zu jedem echten
        // Motor. (Ein async-Generator hat eins; den bauen wir nicht.)
        if !f.is_arrow && !(f.is_async && !f.is_generator) {
            // Das `prototype` einer Generatorfunktion haengt unter
            // `%GeneratorPrototype%` und traegt KEIN `constructor` — von dort
            // erbt das Generatorobjekt `next`/`return`/`throw`. Eine
            // gewoehnliche Funktion bekommt das gewohnte Paar.
            let is_gen = f.is_generator && !f.is_async;
            let proto = new_obj(Some(if is_gen {
                self.realm.generator_proto.clone()
            } else {
                self.realm.object_proto.clone()
            }));
            if !is_gen {
                proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(g.clone())));
            }
            g.borrow_mut().define("prototype", Prop {
                value: Some(Value::Obj(proto)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }
}



/// Die Sicht hinter einem Objekt, wenn es eine ist.
pub fn ta_of(o: &Gc) -> Option<Rc<TaData>> {
    match &o.borrow().kind {
        ObjKind::TypedArray(t) => Some(t.clone()),
        _ => None,
    }
}

/// Ein Element einer Sicht lesen. `None` heisst „das ist keine Sicht oder
/// kein Index" — dann laeuft der gewoehnliche Weg weiter. `Some(Undefined)`
/// heisst „Sicht, aber ausserhalb", und das ist eine ANTWORT, kein Durchfall.
pub fn ta_read(o: &Gc, key: &str) -> Option<Value> {
    let t = ta_of(o)?;
    let k = array_index(key)? as usize;
    if k >= t.live_len() { return Some(Value::Undefined) }
    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Some(Value::Undefined) };
    let at = t.offset + k * t.kind.size();
    Some(t.kind.read_v(&b.bytes.borrow(), at))
}
