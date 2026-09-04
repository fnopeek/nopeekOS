//! Werte und das Objektmodell.
//!
//! **Eigenschaftsbeschreibungen von Anfang an.** Ein Objektmodell, das nur
//! Werte kennt und `writable`/`enumerable`/`configurable` spaeter nachruestet,
//! muss jede Zeile noch einmal anfassen — und test262 fragt danach in fast
//! jedem zweiten Test. Also gleich richtig.
//!
//! **Zaehlende Freigabe, kein Sammler.** `Rc` heisst: Zyklen bleiben liegen.
//! Fuer einen Browser, der Skripte je Seite laufen laesst und die Instanz beim
//! Verlassen wegwirft, ist das tragbar; fuer eine lange laufende Anwendung
//! nicht. Das ist eine bewusste Schuld, keine Nachlaessigkeit — und die Stelle,
//! an der ein Sammler ansetzen wuerde, ist genau `Gc`.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::HashMap;

pub type Gc = Rc<RefCell<Object>>;

#[derive(Clone)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    Sym(Rc<SymData>),
    Obj(Gc),
}

impl Value {
    pub fn str(s: &str) -> Value { Value::Str(Rc::from(s)) }
    pub fn string(s: String) -> Value { Value::Str(Rc::from(s.as_str())) }

    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Bool(_) => "boolean",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::Sym(_) => "symbol",
            Value::Obj(o) => {
                if matches!(o.borrow().kind, ObjKind::Function(_) | ObjKind::Native(_)) {
                    "function"
                } else { "object" }
            }
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Sym(_) => true,
            Value::Obj(_) => true,
        }
    }

    pub fn as_obj(&self) -> Option<&Gc> {
        match self { Value::Obj(o) => Some(o), _ => None }
    }

    /// `===`. Der einzige Haken ist NaN (nie gleich) und `-0 === 0` (gleich).
    pub fn strict_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            // Zwei Symbole sind dasselbe, wenn ihr SCHLUESSEL derselbe ist —
            // und der ist je Symbol einmalig. Damit ist die Identitaet nicht
            // an den `Rc` gebunden, und `Symbol.for` darf ihn frisch bauen.
            (Value::Sym(a), Value::Sym(b)) => a.key == b.key,
            (Value::Obj(a), Value::Obj(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// `Object.is`-Gleichheit: wie `===`, aber NaN ist sich selbst gleich und
    /// `-0` ist nicht `0`. `assert._isSameValue` baut genau das nach.
    pub fn same_value(&self, other: &Value) -> bool {
        if let (Value::Num(a), Value::Num(b)) = (self, other) {
            if a.is_nan() && b.is_nan() { return true; }
            if *a == 0.0 && *b == 0.0 { return a.is_sign_negative() == b.is_sign_negative(); }
        }
        self.strict_eq(other)
    }
}

/// Ein Eigenschaftsname — Zeichenkette ODER Symbol, in EINER Tabelle.
///
/// Ein Symbol traegt seinen Schluessel als Zeichenkette mit fuehrendem
/// NUL-Byte. Das ist kein Trick, um Arbeit zu sparen, sondern die Entscheidung
/// gegen eine zweite Eigenschaftstabelle: mit einem eigenen Schluesseltyp
/// waere jede Suche `HashMap<PropName, _>` und muesste fuer jedes `o.foo`
/// erst einen `PropName` bauen — eine Allokation auf dem heissesten Pfad der
/// Maschine. So bleibt `get_own(&str)` unveraendert und kostenlos.
///
/// Der Preis, ehrlich benannt: eine Seite, die `obj["\0#7"]` schreibt, trifft
/// den Namensraum der Symbole. Kein Zeichen ist von aussen unerreichbar —
/// `String.fromCharCode(0)` gibt es. Ein fuehrendes NUL ist aber die Form,
/// die in echtem Code nicht vorkommt, und die Pruefung kostet EIN Byte
/// (`is_sym_key`), nicht einen Praefixvergleich.
pub type PropName = Rc<str>;

/// Ein Symbol. `key` ist der Eigenschaftsname, unter dem es in Objekten liegt,
/// und zugleich seine Identitaet ([[Value::strict_eq]]).
pub struct SymData {
    pub desc: Option<Rc<str>>,
    pub key: Rc<str>,
    /// Gesetzt bei `Symbol.for` — `Symbol.keyFor` gibt genau das zurueck.
    pub registered: Option<Rc<str>>,
}

/// Gehoert dieser Eigenschaftsname einem Symbol?
#[inline]
pub fn is_sym_key(k: &str) -> bool { k.as_bytes().first() == Some(&0) }

/// Aus einem Symbolschluessel das Symbol zurueckgewinnen.
///
/// Nicht Bequemlichkeit, sondern Notwendigkeit:
/// `Object.getOwnPropertySymbols` gibt SYMBOLE zurueck, und in der Tabelle
/// steht nur der Schluessel. Also traegt der Schluessel alles, was ein Symbol
/// ausmacht — Beschreibung und Registrierung — und ist trotzdem einmalig.
///
///   `\0@iterator`  wohlbekannt  → `Symbol.iterator`
///   `\0*name`      registriert  → `Symbol.for("name")`
///   `\0#7`         anonym       → `Symbol()`
///   `\0#7:text`    beschrieben  → `Symbol("text")`
pub fn sym_from_key(k: &PropName) -> SymData {
    let body = &k[1..];
    let (desc, registered) = match body.as_bytes().first() {
        Some(b'@') => (Some(Rc::from(alloc::format!("Symbol.{}", &body[1..]).as_str())), None),
        Some(b'*') => { let n: Rc<str> = Rc::from(&body[1..]); (Some(n.clone()), Some(n)) }
        _ => match body.find(':') {
            Some(i) => (Some(Rc::from(&body[i + 1..])), None),
            None => (None, None),
        },
    };
    SymData { desc, key: k.clone(), registered }
}

/// Die wohlbekannten Symbole. Ihr Schluessel ist eine Konstante, damit
/// eingebauter Code `self.get(v, SYM_ITERATOR)` schreiben kann, ohne das
/// Symbolobjekt erst zu suchen.
pub const SYM_ITERATOR: &str = "\0@iterator";
pub const SYM_ASYNC_ITERATOR: &str = "\0@asyncIterator";
pub const SYM_HAS_INSTANCE: &str = "\0@hasInstance";
pub const SYM_IS_CONCAT_SPREADABLE: &str = "\0@isConcatSpreadable";
pub const SYM_MATCH: &str = "\0@match";
pub const SYM_MATCH_ALL: &str = "\0@matchAll";
pub const SYM_REPLACE: &str = "\0@replace";
pub const SYM_SEARCH: &str = "\0@search";
pub const SYM_SPECIES: &str = "\0@species";
pub const SYM_SPLIT: &str = "\0@split";
pub const SYM_TO_PRIMITIVE: &str = "\0@toPrimitive";
pub const SYM_TO_STRING_TAG: &str = "\0@toStringTag";
pub const SYM_UNSCOPABLES: &str = "\0@unscopables";

/// Der Zustand eines eingebauten Iterators. Auch NUL-praefigiert, also aus
/// `own_keys` heraus und fuer jedes Skript unsichtbar — `native` nimmt keinen
/// Abschluss, der Zustand muss also irgendwo am Objekt liegen.
pub const IT_TARGET: &str = "\0!target";
pub const IT_INDEX: &str = "\0!index";
/// 0 = Werte, 1 = Schluessel, 2 = Paare.
pub const IT_KIND: &str = "\0!kind";

/// Die Liste, aus der `Symbol.iterator` & Co. am globalen Objekt entstehen.
pub const WELL_KNOWN: &[(&str, &str)] = &[
    ("iterator", SYM_ITERATOR),
    ("asyncIterator", SYM_ASYNC_ITERATOR),
    ("hasInstance", SYM_HAS_INSTANCE),
    ("isConcatSpreadable", SYM_IS_CONCAT_SPREADABLE),
    ("match", SYM_MATCH),
    ("matchAll", SYM_MATCH_ALL),
    ("replace", SYM_REPLACE),
    ("search", SYM_SEARCH),
    ("species", SYM_SPECIES),
    ("split", SYM_SPLIT),
    ("toPrimitive", SYM_TO_PRIMITIVE),
    ("toStringTag", SYM_TO_STRING_TAG),
    ("unscopables", SYM_UNSCOPABLES),
];

#[derive(Clone)]
pub struct Prop {
    pub value: Option<Value>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

impl Prop {
    pub fn data(v: Value) -> Prop {
        Prop { value: Some(v), get: None, set: None, writable: true, enumerable: true, configurable: true }
    }
    /// Was eingebaute Eigenschaften tragen: schreibbar und konfigurierbar,
    /// aber NICHT aufzaehlbar (ES 17). Ein `for..in` ueber ein frisches Objekt
    /// darf `toString` nicht sehen.
    pub fn builtin(v: Value) -> Prop {
        Prop { value: Some(v), get: None, set: None, writable: true, enumerable: false, configurable: true }
    }
    pub fn frozen(v: Value) -> Prop {
        Prop { value: Some(v), get: None, set: None, writable: false, enumerable: false, configurable: false }
    }
    pub fn is_accessor(&self) -> bool { self.get.is_some() || self.set.is_some() }
}

pub type NativeFn = fn(&mut crate::js::interp::Interp, Value, &[Value]) -> Result<Value, crate::js::interp::Abrupt>;

pub struct NativeData {
    pub func: NativeFn,
    pub name: Rc<str>,
    pub length: usize,
    /// Darf mit `new` gerufen werden (Konstruktor)?
    pub ctor: bool,
}

pub struct FuncData {
    pub node: Rc<crate::js::ast::Func>,
    pub env: Rc<RefCell<crate::js::interp::Env>>,
    /// Gebundenes `this` — Pfeile erben es, gewoehnliche Funktionen bekommen
    /// es beim Aufruf.
    pub this_val: Option<Value>,
    pub home_object: Option<Gc>,
}

pub enum ObjKind {
    Plain,
    Array,
    Function(Rc<FuncData>),
    Native(Rc<NativeData>),
    /// Gebundene Funktion aus `Function.prototype.bind`.
    Bound { target: Gc, this_val: Value, args: Vec<Value> },
    Error,
    BoolWrap(bool),
    NumWrap(f64),
    StrWrap(Rc<str>),
    SymWrap(Rc<SymData>),
    Arguments,
    Regex(Rc<crate::js::regexp::Regex>),
    Promise(Rc<RefCell<crate::js::promise::PData>>),
    /// Ein angehaltener Generator: seine EIGENE Maschine, samt Zustand.
    /// Siehe `generator.rs` — und den Kopf von `vm.rs` fuer den Grund, warum
    /// es eine eigene ist und kein Rahmen in einer fremden.
    Generator(Rc<crate::js::generator::GenState>),
}

pub struct Object {
    props: HashMap<PropName, Prop>,
    /// Einfuegereihenfolge. JS gibt Eigenschaften in einer FESTGELEGTEN
    /// Reihenfolge zurueck (ganzzahlige Schluessel aufsteigend zuerst, dann
    /// der Rest in Einfuegereihenfolge) — eine reine Hashtabelle kann das
    /// nicht, also steht die Reihenfolge daneben.
    order: Vec<PropName>,
    pub proto: Option<Gc>,
    pub kind: ObjKind,
    pub extensible: bool,
}

impl Object {
    pub fn new(proto: Option<Gc>) -> Object {
        Object { props: HashMap::new(), order: Vec::new(), proto, kind: ObjKind::Plain, extensible: true }
    }
    pub fn with_kind(proto: Option<Gc>, kind: ObjKind) -> Object {
        Object { props: HashMap::new(), order: Vec::new(), proto, kind, extensible: true }
    }

    pub fn get_own(&self, k: &str) -> Option<&Prop> { self.props.get(k) }
    pub fn has_own(&self, k: &str) -> bool { self.props.contains_key(k) }

    pub fn set_prop(&mut self, k: PropName, p: Prop) {
        if !self.props.contains_key(&k) { self.order.push(k.clone()); }
        self.props.insert(k, p);
    }
    pub fn define(&mut self, k: &str, p: Prop) { self.set_prop(Rc::from(k), p); }

    /// Alle Index-Eigenschaften in EINEM Durchgang entfernen.
    ///
    /// `remove` je Schluessel laeuft die Reihenfolgeliste jedes Mal ab — bei
    /// n Schluesseln also O(n²), und das an der Schrittgrenze vorbei. Genau
    /// daran ist der Lauf von 20 auf ueber 60 Sekunden gestiegen, nachdem
    /// `shift`/`splice`/`sort` dazukamen.
    /// Alles wegnehmen. Nur fuer den Abbau eines Realms — siehe
    /// `Interp::teardown`, dort steht, warum es das braucht.
    pub fn clear_props(&mut self) {
        self.props.clear();
        self.order.clear();
    }

    pub fn clear_indices(&mut self) {
        self.props.retain(|k, _| array_index(k).is_none());
        self.order.retain(|k| array_index(k).is_none());
    }

    pub fn remove(&mut self, k: &str) -> bool {
        if self.props.remove(k).is_some() {
            self.order.retain(|n| &**n != k);
            true
        } else { false }
    }

    /// Eigene ZEICHENKETTEN-Schluessel in der Reihenfolge, die die
    /// Spezifikation vorschreibt: ganzzahlige Indizes aufsteigend, danach
    /// alles andere in Einfuegereihenfolge.
    ///
    /// Symbole sind hier NICHT dabei — jeder Aufrufer (`Object.keys`,
    /// `for..in`, `JSON.stringify`, `getOwnPropertyNames`) will genau das.
    /// Wer Symbole braucht, nimmt `own_sym_keys`.
    pub fn own_keys(&self) -> Vec<PropName> {
        let mut idx: Vec<(u32, PropName)> = Vec::new();
        let mut rest: Vec<PropName> = Vec::new();
        for k in &self.order {
            if is_sym_key(k) { continue; }
            match array_index(k) {
                Some(i) => idx.push((i, k.clone())),
                None => rest.push(k.clone()),
            }
        }
        idx.sort_by_key(|(i, _)| *i);
        let mut out: Vec<PropName> = idx.into_iter().map(|(_, k)| k).collect();
        out.append(&mut rest);
        out
    }

    /// Eigene SYMBOL-Schluessel, in Einfuegereihenfolge.
    pub fn own_sym_keys(&self) -> Vec<PropName> {
        self.order.iter().filter(|k| is_sym_key(k)).cloned().collect()
    }

    pub fn prop_count(&self) -> usize { self.props.len() }
}

/// Ist `k` ein Array-Index? Die Regel ist eng: eine kanonische Dezimalzahl
/// ohne fuehrende Null, kleiner als 2^32-1. `"01"` und `"1.0"` sind KEINE
/// Indizes, und daran haengt die Reihenfolge der Schluessel.
pub fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || k.len() > 10 { return None; }
    if k.len() > 1 && k.starts_with('0') { return None; }
    if !k.bytes().all(|b| b.is_ascii_digit()) { return None; }
    k.parse::<u32>().ok().filter(|v| *v < u32::MAX)
}

/// Zahl zu Zeichenkette, nach den Regeln von JS.
///
/// Nicht `format!("{}")`: Rust schreibt `1` als `1` (gut), aber auch `1e21`
/// als `1000000000000000000000` und `f64::INFINITY` als `inf`. JS hat eigene
/// Regeln, und Zahlen werden staendig zu Text.
pub fn num_to_string(n: f64) -> String {
    if n.is_nan() { return "NaN".to_string(); }
    if n == f64::INFINITY { return "Infinity".to_string(); }
    if n == f64::NEG_INFINITY { return "-Infinity".to_string(); }
    if n == 0.0 { return "0".to_string(); }
    if n == libm::trunc(n) && libm::fabs(n) < 1e21 {
        // Ganzzahlig und im Bereich, in dem JS keine Exponenten benutzt.
        let mut s = String::new();
        let neg = n < 0.0;
        let mut v = libm::fabs(n);
        let mut digits = Vec::new();
        while v >= 1.0 {
            let d = (v % 10.0) as u8;
            digits.push(b'0' + d);
            v = libm::trunc(v / 10.0);
        }
        if neg { s.push('-'); }
        for d in digits.iter().rev() { s.push(*d as char); }
        return s;
    }
    // Der allgemeine Fall. Rusts kuerzeste Darstellung stimmt mit der von JS
    // fuer den Bereich ueberein, in dem beide keine Exponentialform waehlen;
    // darueber schreibt JS `1e+21`, Rust `1000…`. Deshalb die Grenze oben.
    let mut s = alloc::format!("{}", n);
    if s.contains('e') && !s.contains("e-") { s = s.replace('e', "e+"); }
    s
}

/// Zeichenkette zu Zahl (`Number("…")`). Leerraum ringsum, `0x`/`0o`/`0b`,
/// `Infinity`, und leer = 0.
pub fn string_to_num(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\u{feff}' || c == '\u{a0}');
    if t.is_empty() { return 0.0; }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).map(|v| v as f64).unwrap_or(f64::NAN);
    }
    if let Some(h) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return u64::from_str_radix(h, 8).map(|v| v as f64).unwrap_or(f64::NAN);
    }
    if let Some(h) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return u64::from_str_radix(h, 2).map(|v| v as f64).unwrap_or(f64::NAN);
    }
    match t {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => t.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// `ToInt32` — die Umwandlung hinter den Bitoperatoren. Modulo 2^32 mit
/// Vorzeichen; NaN und Unendlich werden 0.
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 { return 0; }
    let m = libm::trunc(n) % 4294967296.0;
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    if m >= 2147483648.0 { (m - 4294967296.0) as i32 } else { m as i32 }
}
pub fn to_uint32(n: f64) -> u32 { to_int32(n) as u32 }

/// `ToInteger`: abschneiden, NaN zu 0.
pub fn to_integer(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else if n.is_infinite() { n } else { libm::trunc(n) }
}

pub fn new_obj(proto: Option<Gc>) -> Gc { Rc::new(RefCell::new(Object::new(proto))) }
pub fn new_kind(proto: Option<Gc>, kind: ObjKind) -> Gc {
    Rc::new(RefCell::new(Object::with_kind(proto, kind)))
}

/// Damit `Box<dyn …>` nicht noetig ist, wenn ein natives Objekt gebaut wird.
pub fn native(proto: Option<Gc>, f: NativeFn, name: &str, length: usize, ctor: bool) -> Gc {
    let g = new_kind(proto, ObjKind::Native(Rc::new(
        NativeData { func: f, name: Rc::from(name), length, ctor })));
    {
        let mut o = g.borrow_mut();
        o.define("length", Prop { value: Some(Value::Num(length as f64)), get: None, set: None,
            writable: false, enumerable: false, configurable: true });
        o.define("name", Prop { value: Some(Value::str(name)), get: None, set: None,
            writable: false, enumerable: false, configurable: true });
    }
    g
}

