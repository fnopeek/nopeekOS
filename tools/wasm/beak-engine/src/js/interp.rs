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
}

impl Env {
    pub fn new(parent: Option<Rc<RefCell<Env>>>, func_scope: bool) -> Rc<RefCell<Env>> {
        Rc::new(RefCell::new(Env {
            vars: HashMap::new(), parent, this_val: None, is_func_scope: func_scope, home: None,
        }))
    }
}

pub fn env_lookup(env: &Rc<RefCell<Env>>, name: &str) -> Option<Rc<RefCell<Env>>> {
    let mut cur = env.clone();
    loop {
        if cur.borrow().vars.contains_key(name) { return Some(cur); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return None }
    }
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
    /// Die Schnittstellen-Prototypen der DOM-Bindung. `tag_protos` bildet den
    /// Elementnamen auf seine Schnittstelle ab; was nicht darinsteht, ist
    /// `HTMLElement`.
    pub html_element_proto: Gc,
    pub svg_element_proto: Gc,
    pub fragment_proto: Gc,
    pub tag_protos: HashMap<&'static str, Gc>,
    pub url_proto: Gc,
    pub url_params_proto: Gc,
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

pub struct Interp {
    pub realm: Realm,
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
    /// Eine Uhr, die nur steigt. Ersatz, bis beak die echte einreicht —
    /// `beak-engine` ist hostfrei und hat keine.
    pub fake_now: f64,
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
    pub cookies: String,
    /// Was die Seite mit `document.cookie = "…"` gesetzt hat, roh und in der
    /// Reihenfolge. Der Wirt holt es sich mit `take_cookie_sets` und legt es
    /// in seinen Behaelter — die Engine entscheidet nicht, was gilt.
    pub cookie_sets: Vec<String>,
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
    pub console: Vec<String>,
    console_dropped: usize,
}

/// Wie viele Zeilen `console` haelt, und wie lang eine werden darf.
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
        super::url::install(&mut realm);
        Interp { realm, depth: 0, max_depth: MAX_DEPTH, steps: 0, max_steps: u64::MAX,
                 fake_now: 0.0, doc: None, next_sym: 0, sym_registry: HashMap::new(),
                 cookies: String::new(), cookie_sets: Vec::new(), style_ctx: None,
                 vm_ran: 0, vm_declined: 0, vm_decline: None, vm_off: false,
                 func_chunks: HashMap::new(), func_declines: HashMap::new(), pending_labels: Vec::new(), vm_calls: 0, vm_calls_native: 0, vm_calls_slow: 0,
                 geometry: None,
                 live_dom: core::cell::RefCell::new(None),
                 jobs: alloc::collections::VecDeque::new(),
                 rng: 0x2545_F491_4F6C_DD1D, media: None,
                 timers: Vec::new(), console: Vec::new(), console_dropped: 0 }
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
            let _ = self.call(&f, Value::Undefined, &[]);
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
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        // `Symbol.toPrimitive` geht VOR `valueOf`/`toString` — es ist der
        // einzige Weg, auf dem ein Objekt beide ueberstimmen kann.
        let exotic = self.get(v, SYM_TO_PRIMITIVE)?;
        if self.is_callable(&exotic) {
            let hint = Value::str(if hint_string { "string" } else { "number" });
            let r = self.call(&exotic, v.clone(), &[hint])?;
            if !matches!(r, Value::Obj(_)) { return Ok(r); }
            return self.type_err("Symbol.toPrimitive returned an object");
        }
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
            Value::Obj(_) => { let p = self.to_primitive(v, false)?; self.to_number(&p)? }
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
            Value::Undefined | Value::Null =>
                self.type_err("cannot convert undefined or null to object"),
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        matches!(v, Value::Obj(o) if matches!(o.borrow().kind,
            ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. }))
    }

    // ── Eigenschaften ────────────────────────────────────────────────────
    pub fn get(&mut self, base: &Value, key: &str) -> C<Value> {
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
                    return self.call(&g.clone(), base.clone(), &[]);
                }
                if p.is_accessor() { return Ok(Value::Undefined); }
                return Ok(p.value.clone().unwrap_or(Value::Undefined));
            }
            let next = o.borrow().proto.clone();
            cur = next;
        }
        Ok(Value::Undefined)
    }

    pub fn set(&mut self, base: &Value, key: &str, val: Value) -> C<()> {
        let Value::Obj(o) = base else {
            // Zuweisung an eine Eigenschaft eines Primitivs verpufft still
            // (im lockeren Modus). Der strenge Modus wuerde werfen — das
            // gehoert zu den Dingen, die der Lauf als offen ausweist.
            return Ok(());
        };
        // Ein Setzer irgendwo in der Kette gewinnt vor dem eigenen Feld.
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = c.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(s) = &p.set { self.call(&s.clone(), base.clone(), &[val])?; return Ok(()); }
                if p.is_accessor() { return Ok(()); }        // nur Getter: still
                if Rc::ptr_eq(&c, o) {
                    if !p.writable { return Ok(()); }
                    let mut np = p.clone();
                    np.value = Some(val);
                    o.borrow_mut().set_prop(Rc::from(key), np);
                    return Ok(());
                }
                if !p.writable { return Ok(()); }
                break;
            }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        if !o.borrow().extensible { return Ok(()); }
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
        if !d.node.is_arrow {
            env.borrow_mut().this_val = Some(d.this_val.clone().unwrap_or(this_val));
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
        match self.run_body(&d.node.body, &env) {
            Ok(()) => Ok(Value::Undefined),
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
    pub fn to_prop_desc(&mut self, d: &Value) -> C<Prop> {
        let Value::Obj(dd) = d else {
            return self.type_err("property descriptor must be an object");
        };
        let has = |n: &str| dd.borrow().has_own(n);
        let get = self.get(d, "get")?;
        let set = self.get(d, "set")?;
        Ok(Prop {
            value: if has("value") { Some(self.get(d, "value")?) } else { None },
            get: if matches!(get, Value::Undefined) { None } else { Some(get) },
            set: if matches!(set, Value::Undefined) { None } else { Some(set) },
            writable: self.get(d, "writable")?.truthy(),
            enumerable: self.get(d, "enumerable")?.truthy(),
            configurable: self.get(d, "configurable")?.truthy(),
        })
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
            .filter(|k| src.borrow().get_own(k).map(|p| p.enumerable).unwrap_or(false))
            .collect();
        for k in keys {
            let d = self.get(props, &k)?;
            let p = self.to_prop_desc(&d)?;
            target.borrow_mut().set_prop(k, p);
        }
        Ok(())
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
            for k in c.borrow().own_keys() {
                let enumerable = c.borrow().get_own(&k).map(|p| p.enumerable).unwrap_or(false);
                if enumerable && !keys.iter().any(|x| *x == k) { keys.push(k); }
            }
            let next = c.borrow().proto.clone();
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


