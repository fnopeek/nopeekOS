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
    /// Eine ganze Zahl ohne Groessengrenze. Ein PRIMITIV, kein Objekt.
    BigInt(Rc<crate::js::bigint::Big>),
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
            Value::BigInt(_) => "bigint",
            Value::Obj(o) => {
                // Eine GEBUNDENE Funktion ist eine, und ein Stellvertreter ist
                // eine, wenn sein Ziel eine ist. Beides fehlte hier.
                let kind = &o.borrow().kind;
                match kind {
                    ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => "function",
                    ObjKind::Proxy(c) => match c.borrow().clone() {
                        Some((t, _)) => Value::Obj(t).type_of(),
                        None => "object",
                    },
                    _ => "object",
                }
            }
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::BigInt(b) => !b.is_zero(),
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
            (Value::BigInt(a), Value::BigInt(b)) => a == b,
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

/// Der Schluessel, unter dem ein privates Feld `#name` liegt.
///
/// Das NUL davor ist der ganze Trick: solche Schluessel fallen aus
/// `own_keys` heraus, und damit ist ein privates Feld fuer `Object.keys`,
/// `for..in`, `JSON.stringify` und die Streuung unsichtbar — ohne einen
/// zweiten Speicher neben der Eigenschaftstabelle.
///
/// **Was das NICHT leistet:** zwei Klassen, die beide ein `#p` auf DEMSELBEN
/// Objekt anlegen, teilen es sich. Echte Motoren schluesseln nach Klasse.
/// Der Fall verlangt, dass eine Klasse ein fremdes Objekt als `this`
/// bekommt; die Vereinfachung ist benannt und nicht still.
/// Liegt hier ein privates Feld?
///
/// Der Schluessel eines Symbols faengt ebenfalls mit NUL an — die beiden zu
/// trennen ist Pflicht, sonst gibt `Object.getOwnPropertySymbols` das private
/// Feld als Symbol heraus, und damit ist es nicht mehr privat. Genau das ist
/// beim ersten Lauf passiert.
///
/// **Und das zweite Zeichen ist deshalb `~` und nicht `#`:** ein
/// gewoehnliches Symbol liegt schon unter `\0#<n>:<beschreibung>`
/// (`Interp::new_symbol`). Der erste Entwurf nahm `#`, und damit verschwanden
/// die echten Symbole aus `getOwnPropertySymbols` — ein Feld fuer Neues, das
/// das Alte umbringt. Die Marker sind: `@` wohlbekannt, `*` registriert,
/// `#` gewoehnlich, `~` privat.
pub fn is_private_key(k: &str) -> bool { k.as_bytes().starts_with(b"\0~") }

pub fn private_key(name: &str) -> Rc<str> {
    Rc::from(alloc::format!("{PRIVATE_PREFIX}{name}").as_str())
}

/// Das Vorzeichen eines privaten Feldes im Schluesseltext. Ein Skript kann
/// es nicht erzeugen — NUL steht in keinem Bezeichner —, also ist „faengt
/// damit an" ein sicheres Erkennungsmerkmal fuer die Markenpruefung.
pub const PRIVATE_PREFIX: &str = "\0~";

/// Der Name ohne Vorzeichen, fuer die Fehlermeldung.
pub fn private_name(key: &str) -> &str {
    key.strip_prefix(PRIVATE_PREFIX).unwrap_or(key)
}

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
pub const SYM_DISPOSE: &str = "\0@dispose";
pub const SYM_ASYNC_DISPOSE: &str = "\0@asyncDispose";

/// Der Zustand eines eingebauten Iterators. Auch NUL-praefigiert, also aus
/// `own_keys` heraus und fuer jedes Skript unsichtbar — `native` nimmt keinen
/// Abschluss, der Zustand muss also irgendwo am Objekt liegen.
pub const IT_TARGET: &str = "\0!target";
/// Der Ersatz fuer das interne Feld `[[SetData]]`/`[[MapData]]`: welche
/// Sammlung das hier IST. NUL-praefigiert, also fuer jedes Skript unsichtbar
/// — ohne den Vermerk waere eine selbstgebaute Menge von einer echten nicht
/// zu unterscheiden, und `Set.prototype.union.call({…})` liefe still durch.
pub const COLL_KIND: &str = "\0!coll";
pub const EV_REASON: &str = "\0!ev.reason";
pub const EV_PROMISE: &str = "\0!ev.promise";
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
    ("dispose", SYM_DISPOSE),
    ("asyncDispose", SYM_ASYNC_DISPOSE),
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

/// Ein PARTIELLER Beschreiber — was `Object.defineProperty` bekommt.
///
/// **Nicht dasselbe wie `Prop`, und das ist der ganze Punkt.** `Prop` ist
/// eine fertig abgelegte Eigenschaft, dort ist `writable` ein `bool`. Ein
/// Beschreiber dagegen hat FEHLENDE Felder, und „fehlt" heisst „lass, wie es
/// ist" — nicht „false". Bis 0.99.0 fielen beide auf `Prop` zusammen, und
/// damit war `defineProperty(o,'x',{value:1})` auf einer schreibbaren
/// Eigenschaft ein stilles `writable = false`.
///
/// `Some(Value::Undefined)` heisst „steht da und ist `undefined`" und ist
/// etwas anderes als `None` — `{get: undefined}` macht eine
/// Zugriffseigenschaft ohne Leser.
#[derive(Clone, Default)]
pub struct Desc {
    pub value: Option<Value>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub writable: Option<bool>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl Desc {
    pub fn is_accessor(&self) -> bool { self.get.is_some() || self.set.is_some() }
    pub fn is_data(&self) -> bool { self.value.is_some() || self.writable.is_some() }
    /// Weder das eine noch das andere — `{enumerable: true}` allein.
    pub fn is_generic(&self) -> bool { !self.is_accessor() && !self.is_data() }
    pub fn is_empty(&self) -> bool {
        self.is_generic() && self.enumerable.is_none() && self.configurable.is_none()
    }
    /// Der Beschreiber einer BESTEHENDEN Eigenschaft: vollstaendig besetzt.
    pub fn from_prop(p: &Prop) -> Desc {
        if p.is_accessor() {
            Desc { value: None, writable: None,
                   get: Some(p.get.clone().unwrap_or(Value::Undefined)),
                   set: Some(p.set.clone().unwrap_or(Value::Undefined)),
                   enumerable: Some(p.enumerable), configurable: Some(p.configurable) }
        } else {
            Desc { value: Some(p.value.clone().unwrap_or(Value::Undefined)),
                   writable: Some(p.writable), get: None, set: None,
                   enumerable: Some(p.enumerable), configurable: Some(p.configurable) }
        }
    }
    /// Was daraus wird, wenn die Eigenschaft NEU angelegt wird: jedes
    /// fehlende Feld bekommt seinen Vorgabewert, und der ist `false`/
    /// `undefined` — nur HIER, nicht beim Aendern.
    pub fn into_new_prop(self) -> Prop {
        Prop {
            value: if self.is_accessor() { None }
                   else { Some(self.value.unwrap_or(Value::Undefined)) },
            // **`undefined` bleibt stehen.** `{set: undefined}` ist eine
            // ZUGRIFFSeigenschaft ohne Schreiber — filtert man das auf `None`,
            // sieht sie hinterher aus wie eine Datenbeschreibung, und ein
            // erneutes Umdefinieren wird faelschlich abgelehnt.
            get: self.get,
            set: self.set,
            writable: self.writable.unwrap_or(false),
            enumerable: self.enumerable.unwrap_or(false),
            configurable: self.configurable.unwrap_or(false),
        }
    }
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
    /// Was ein `@@toStringTag` (und `Symbol.prototype[@@toPrimitive]`) traegt:
    /// nicht schreibbar, nicht aufzaehlbar, aber **konfigurierbar** (ES 17).
    /// `Prop::frozen` war dafuer der falsche Konstruktor — er sperrt auch das
    /// Umdefinieren, und das faellt erst auf, wenn `defineProperty` wirklich
    /// prueft.
    pub fn tag(v: Value) -> Prop {
        Prop { value: Some(v), get: None, set: None, writable: false, enumerable: false, configurable: true }
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
    /// Die Klasse, deren KONSTRUKTOR das hier ist — und nur dann gesetzt,
    /// wenn sie Instanzfelder hat.
    ///
    /// Ein Instanzfeld gehoert weder auf den Prototyp (es ist je Instanz)
    /// noch in den Rumpf (es steht dort nicht). Es gehoert an den Aufruf, und
    /// der Aufruf braucht dafuer die Liste — hier liegt sie. `env` daneben ist
    /// schon der richtige Bereich fuer die Initialisierer.
    pub class: Option<Rc<crate::js::ast::Class>>,
}

/// Der Bytespeicher hinter jedem TypedArray und jeder DataView.
///
/// **Ein `ArrayBuffer` ist der Speicher, ein TypedArray nur eine SICHT
/// darauf.** Zwei Sichten auf denselben Puffer sehen einander — das ist der
/// Punkt der ganzen Familie, und deshalb liegt der Speicher hier und nicht
/// in der Sicht.
pub struct BufData {
    pub bytes: RefCell<alloc::vec::Vec<u8>>,
    /// Abgetrennt. Danach hat der Puffer die Laenge 0 und jede Sicht darauf
    /// ist leer; test262 loest das ueber `$262.detachArrayBuffer` aus.
    pub detached: core::cell::Cell<bool>,
}

/// Die Elementart einer Sicht.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElemKind { I8, U8, U8C, I16, U16, I32, U32, F32, F64, I64, U64 }

impl ElemKind {
    pub fn size(self) -> usize {
        match self {
            ElemKind::I8 | ElemKind::U8 | ElemKind::U8C => 1,
            ElemKind::I16 | ElemKind::U16 => 2,
            ElemKind::I32 | ElemKind::U32 | ElemKind::F32 => 4,
            ElemKind::F64 | ElemKind::I64 | ElemKind::U64 => 8,
        }
    }
    /// Traegt diese Art GROSSE Zahlen? Dann ist ein Element ein `BigInt`, und
    /// jeder Schreiber muss `ToBigInt` statt `ToNumber` rufen.
    pub fn is_big(self) -> bool { matches!(self, ElemKind::I64 | ElemKind::U64) }
    pub fn name(self) -> &'static str {
        match self {
            ElemKind::I8 => "Int8Array", ElemKind::U8 => "Uint8Array",
            ElemKind::U8C => "Uint8ClampedArray", ElemKind::I16 => "Int16Array",
            ElemKind::U16 => "Uint16Array", ElemKind::I32 => "Int32Array",
            ElemKind::U32 => "Uint32Array", ElemKind::F32 => "Float32Array",
            ElemKind::F64 => "Float64Array",
            ElemKind::I64 => "BigInt64Array", ElemKind::U64 => "BigUint64Array",
        }
    }

    /// Ein Element als JS-Wert — bei den 64-Bit-Arten eine grosse Zahl.
    pub fn read_v(self, b: &[u8], at: usize) -> Value {
        if !self.is_big() { return Value::Num(self.read(b, at)); }
        let mut raw = 0u64;
        for k in 0..8 { raw |= (b[at + k] as u64) << (8 * k); }
        let big = if matches!(self, ElemKind::I64) {
            crate::js::bigint::Big::from_i64(raw as i64)
        } else {
            crate::js::bigint::Big::from_u64(raw)
        };
        Value::BigInt(alloc::rc::Rc::new(big))
    }

    /// Eine grosse Zahl schreiben — abgeschnitten auf 64 Bit.
    pub fn write_big(self, b: &mut [u8], at: usize, v: &crate::js::bigint::Big) {
        let raw = v.to_u64_wrap();
        for k in 0..8 { b[at + k] = ((raw >> (8 * k)) & 0xff) as u8; }
    }
    /// Ein Element lesen. Immer LITTLE ENDIAN — das ist, was jede Plattform
    /// tut, auf der dieser Code laeuft, und die Spec laesst der Sicht (anders
    /// als der `DataView`) keine Wahl.
    pub fn read(self, b: &[u8], at: usize) -> f64 {
        let g = |n: usize| -> u64 {
            let mut v = 0u64;
            for k in 0..n { v |= (b[at + k] as u64) << (8 * k); }
            v
        };
        match self {
            ElemKind::I8 => b[at] as i8 as f64,
            ElemKind::U8 | ElemKind::U8C => b[at] as f64,
            ElemKind::I16 => g(2) as u16 as i16 as f64,
            ElemKind::U16 => g(2) as u16 as f64,
            ElemKind::I32 => g(4) as u32 as i32 as f64,
            ElemKind::U32 => g(4) as u32 as f64,
            ElemKind::F32 => f32::from_bits(g(4) as u32) as f64,
            ElemKind::F64 => f64::from_bits(g(8)),
            // Ueber `read_v` zu holen; hier nur, damit der Uebersetzer die
            // Vollstaendigkeit prueft.
            ElemKind::I64 => g(8) as i64 as f64,
            ElemKind::U64 => g(8) as f64,
        }
    }
    /// Ein Element schreiben. Die Umwandlung ist die der Spec: NaN und
    /// Unendlich werden bei den Ganzzahlen zu 0, der Rest wird modulo
    /// abgeschnitten — ausser bei `Uint8Clamped`, das KLEMMT und rundet.
    pub fn write(self, b: &mut [u8], at: usize, v: f64) {
        let put = |b: &mut [u8], n: usize, raw: u64| {
            for k in 0..n { b[at + k] = ((raw >> (8 * k)) & 0xff) as u8; }
        };
        match self {
            ElemKind::U8C => {
                b[at] = if v.is_nan() { 0 } else if v <= 0.0 { 0 } else if v >= 255.0 { 255 }
                        else {
                            // Zur naechsten GERADEN bei .5 — die Spec sagt das
                            // ausdruecklich, und `round` taete es nicht.
                            let f = libm::floor(v);
                            let d = v - f;
                            let r = if d < 0.5 { f } else if d > 0.5 { f + 1.0 }
                                    else if (f as i64) % 2 == 0 { f } else { f + 1.0 };
                            to_uint32(r) as u8
                        };
            }
            ElemKind::F32 => put(b, 4, (v as f32).to_bits() as u64),
            ElemKind::F64 => put(b, 8, v.to_bits()),
            _ => {
                let n = self.size();
                let m = to_uint_wrap(v, n * 8);
                put(b, n, m);
            }
        }
    }
}

/// `ToIntegerOrInfinity` + modulo 2^bits — der gemeinsame Rumpf von
/// `ToInt8`/`ToUint8`/`ToInt16`/… Ein eigener Name, weil die Regel
/// (NaN und Unendlich zu 0, dann abschneiden, dann modulo) an neun Stellen
/// dieselbe ist.
pub fn to_uint_wrap(v: f64, bits: usize) -> u64 {
    // **Ueber `to_uint32`, nicht ueber einen eigenen f64→u64-Cast.** Der
    // uebersetzt zu `i64.trunc_sat_f64_u`, und forge kann diesen Befehl
    // nicht — ein Modul mit ihm bliebe AM GERAET beim ersten Aufruf der
    // Stelle stehen, nicht beim Laden. Das forge-Tor hat ihn gemeldet, bevor
    // irgendetwas signiert war; ohne das Tor waere er in einer Freigabe
    // gelandet und erst beim Benutzen aufgefallen.
    //
    // Die Rechnung ist dieselbe: `ToUint32` schneidet ab und rechnet modulo
    // 2^32, und alles darunter ist ein Ausschnitt davon.
    let full = to_uint32(v) as u64;
    if bits >= 32 { full } else { full & ((1u64 << bits) - 1) }
}

/// Eine SICHT auf einen Puffer.
pub struct TaData {
    pub buf: Gc,
    pub kind: ElemKind,
    /// Byteversatz im Puffer.
    pub offset: usize,
    /// Anzahl ELEMENTE, nicht Bytes.
    pub len: usize,
}

/// Die ungetypte Sicht: jeder Zugriff nennt seine Art und seine Bytefolge
/// selbst. Deshalb steht hier keine `ElemKind`.
pub struct DvData {
    pub buf: Gc,
    pub offset: usize,
    pub len: usize,
}

impl TaData {
    /// Wieviele Elemente die Sicht WIRKLICH hat — ein abgetrennter Puffer
    /// macht sie leer, ohne dass jemand die Sicht anfasst.
    pub fn live_len(&self) -> usize {
        let ObjKind::Buffer(b) = &self.buf.borrow().kind else { return 0 };
        if b.detached.get() { return 0 }
        let have = b.bytes.borrow().len();
        if self.offset + self.len * self.kind.size() > have { return 0 }
        self.len
    }
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
    BigWrap(Rc<crate::js::bigint::Big>),
    Arguments,
    Regex(Rc<crate::js::regexp::Regex>),
    Promise(Rc<RefCell<crate::js::promise::PData>>),
    /// Der Speicher (`ArrayBuffer`) und die zwei Sichten darauf.
    Buffer(Rc<BufData>),
    TypedArray(Rc<TaData>),
    DataView(Rc<DvData>),
    /// Ein Stellvertreter: Ziel und Behandler, oder `None` nach dem
    /// Widerruf. Siehe `proxy.rs` — die Fallen sitzen in den
    /// Grundoperationen, nicht hier.
    Proxy(crate::js::proxy::ProxyCell),
    /// Der Zeitwert eines `Date`. Als eigene Art und nicht als Eigenschaft,
    /// damit er nicht in `Object.getOwnPropertyNames` auftaucht.
    Date(Rc<core::cell::Cell<f64>>),
    /// Ein angehaltener Generator: seine EIGENE Maschine, samt Zustand.
    /// Siehe `generator.rs` — und den Kopf von `vm.rs` fuer den Grund, warum
    /// es eine eigene ist und kein Rahmen in einer fremden.
    Generator(Rc<crate::js::generator::GenState>),
    /// Der Inhalt einer `Map`/`Set`/`WeakMap`/`WeakSet`.
    Collection(Rc<RefCell<CollData>>),
}

/// Ein Sammlungsschluessel, so wie SameValueZero ihn vergleicht.
///
/// **Warum eine eigene Darstellung.** Bis 0.103.0 lag ein Eintrag als
/// Eigenschaft `@<Zeichenkette des Schluessels>` im Objekt. Das hat drei
/// Dinge falsch gemacht, und alle drei sind an echtem Code aufgefallen: zwei
/// verschiedene Objekte mit gleichem `toString` waren EIN Eintrag, `1` und
/// `"1"` waren derselbe Schluessel, und schon das Nachschlagen RIEF das
/// `toString` des Schluessels. core-js legt seinen internen Zustand in einer
/// `WeakMap` ab, deren Schluessel Funktionen sind — der Aufruf ging in deren
/// `Function.prototype.toString`, das core-js im selben Modul gerade ersetzt
/// hatte, und von dort im Kreis, bis der Aufrufdeckel griff.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CollKey {
    Undefined,
    Null,
    Bool(bool),
    /// Die Bits der Zahl — mit `-0` auf `+0` gelegt und JEDEM NaN auf
    /// dasselbe Muster. Das ist der einzige Unterschied zwischen
    /// SameValueZero und `Object.is`.
    Num(u64),
    Str(Rc<str>),
    Sym(Rc<str>),
    Big(String),
    /// Die ADRESSE, nicht der Inhalt: zwei gleich aussehende Objekte sind
    /// zwei Schluessel. Sie bleibt gueltig, weil `entries` den Schluessel
    /// selbst festhaelt — die Sammlung haelt ihn also am Leben.
    Obj(usize),
}

impl CollKey {
    pub fn of(v: &Value) -> CollKey {
        match v {
            Value::Undefined => CollKey::Undefined,
            Value::Null => CollKey::Null,
            Value::Bool(b) => CollKey::Bool(*b),
            Value::Num(n) => CollKey::Num(
                if n.is_nan() { 0x7ff8_0000_0000_0000 }
                else if *n == 0.0 { 0 }
                else { n.to_bits() }),
            Value::Str(s) => CollKey::Str(s.clone()),
            Value::Sym(s) => CollKey::Sym(s.key.clone()),
            Value::BigInt(b) => CollKey::Big(b.to_string_radix(10)),
            Value::Obj(o) => CollKey::Obj(Rc::as_ptr(o) as *const () as usize),
        }
    }
}

/// Die Eintraege einer Sammlung.
///
/// **Weich, nicht schwach.** `WeakMap` haelt hier genauso fest wie `Map`. Ein
/// echtes schwaches Halten braucht einen Sammler, und den gibt es nicht
/// (siehe Modulkopf) — die Alternative waere ein Schluessel, der still
/// verschwindet, und das waere schlimmer als einer, der zu lange lebt.
#[derive(Default)]
pub struct CollData {
    /// Die Eintraege in EINFUEGEREIHENFOLGE. Geloescht heisst `None` statt
    /// entfernt: `forEach` und die Iteratoren laufen ueber Indizes, und ein
    /// Zusammenschieben wuerde sie mitten im Lauf verschieben — genau der
    /// Fall, den die Spezifikation ausdruecklich regelt.
    pub entries: Vec<Option<(Value, Value)>>,
    /// Schluessel -> Index. Ohne ihn waere jedes `get` ein Durchlauf, und
    /// core-js fragt seinen Zustand bei JEDEM Zugriff.
    pub index: HashMap<CollKey, usize>,
}

impl CollData {
    pub fn get(&self, k: &Value) -> Option<Value> {
        let i = *self.index.get(&CollKey::of(k))?;
        self.entries.get(i).and_then(|e| e.as_ref()).map(|(_, v)| v.clone())
    }
    pub fn has(&self, k: &Value) -> bool { self.index.contains_key(&CollKey::of(k)) }
    /// Setzen. Ein vorhandener Schluessel behaelt seinen PLATZ — die
    /// Reihenfolge richtet sich nach dem ersten Einfuegen.
    pub fn set(&mut self, k: Value, v: Value) {
        let ck = CollKey::of(&k);
        if let Some(&i) = self.index.get(&ck) {
            if let Some(slot) = self.entries.get_mut(i) { *slot = Some((k, v)); return; }
        }
        self.index.insert(ck, self.entries.len());
        self.entries.push(Some((k, v)));
    }
    pub fn remove(&mut self, k: &Value) -> bool {
        let Some(i) = self.index.remove(&CollKey::of(k)) else { return false };
        if let Some(slot) = self.entries.get_mut(i) { *slot = None; }
        true
    }
    pub fn clear(&mut self) { self.entries.clear(); self.index.clear(); }
    pub fn len(&self) -> usize { self.index.len() }
    pub fn is_empty(&self) -> bool { self.index.is_empty() }
    /// Die lebenden Paare als Liste — die Momentaufnahme, aus der `keys`,
    /// `values` und `entries` ihren Iterator bauen.
    pub fn pairs(&self) -> Vec<(Value, Value)> {
        self.entries.iter().flatten().cloned().collect()
    }
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

    /// Ist dieser eigene Schluessel aufzaehlbar?
    ///
    /// **Eigene Funktion, weil die Antwort nicht immer in der Tabelle steht.**
    /// Die Indizes einer Sicht (`TypedArray`) sind aufzaehlbar, obwohl es zu
    /// ihnen keinen Eintrag gibt — sie entstehen aus der Laenge. Acht Stellen
    /// haben das frueher mit `get_own(k).map(|p| p.enumerable)` gefragt und
    /// bekamen fuer eine Sicht achtmal `false`: `Object.keys`, `for..in`,
    /// `JSON.stringify` und die Streuung sahen ein leeres Objekt.
    pub fn is_enumerable(&self, k: &str) -> bool {
        if let Some(p) = self.props.get(k) { return p.enumerable }
        matches!(&self.kind, ObjKind::TypedArray(t)
                 if array_index(k).is_some_and(|x| (x as usize) < t.live_len()))
    }
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
    /// JEDER Schluessel, auch die NUL-praefigierten. `own_keys` laesst die
    /// absichtlich weg — wer aber ein Objekt UEBERNIMMT (`super()` auf einen
    /// eingebauten Konstruktor), braucht auch die inneren Vermerke.
    pub fn raw_keys(&self) -> Vec<PropName> { self.order.clone() }

    pub fn own_keys(&self) -> Vec<PropName> {
        // **Eine Sicht traegt ihre Indizes nicht in der Tabelle.** Sie
        // entstehen aus der Laenge, und ohne diesen Zweig faende
        // `Object.keys(ta)` nichts — auch `for..in` und `JSON.stringify`
        // nicht.
        if let ObjKind::TypedArray(t) = &self.kind {
            let n = t.live_len();
            let mut out: Vec<PropName> = (0..n)
                .map(|k| PropName::from(num_to_string(k as f64).as_str())).collect();
            for k in &self.order {
                if is_sym_key(k) || array_index(k).is_some() { continue }
                out.push(k.clone());
            }
            return out;
        }
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
        self.order.iter().filter(|k| is_sym_key(k) && !is_private_key(k)).cloned().collect()
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
    // **Schnellweg: eine ganze Zahl.** Ein Index, eine Laenge, ein Zaehler —
    // das ist der ueberwiegende Fall, und die Antwort ist die Ziffernfolge.
    //
    // Der Weg darunter zahlt dafuer `format!("{:e}")`: einen Haldenpuffer,
    // Grisu, ein Filtern ueber die Ziffern und einen zweiten Puffer. Auf
    // einer Google-Suchseite waren das **48 % der Laufzeit** — nicht weil
    // die Seite mehr rechnet als in einem Browser, sondern weil jede Zahl,
    // die zu einem Eigenschaftsschluessel wird, diesen Weg nahm.
    //
    // Bis 2^53 ist eine ganze Zahl in `f64` exakt, und unter 1e21 schreibt
    // JS sie schlicht aus (ES 6.1.6.1.20, Fall `1 <= pt <= 21`) — die beiden
    // Bedingungen decken sich also, und der lange Weg bleibt fuer alles
    // andere zustaendig.
    if libm::fabs(n) < 9007199254740992.0 && libm::trunc(n) == n {
        let mut v = n as i64;
        let neg = v < 0;
        if neg { v = -v; }
        let mut d = [0u8; 20];
        let mut k = d.len();
        while v > 0 { k -= 1; d[k] = b'0' + (v % 10) as u8; v /= 10; }
        let mut s = String::with_capacity(d.len() - k + neg as usize);
        if neg { s.push('-'); }
        for &b in &d[k..] { s.push(b as char); }
        return s;
    }
    // ES 6.1.6.1.20 waehlt die Form nach der KOMMASTELLE, nicht nach der
    // Groesse: `s * 10^(n-k)` mit der kuerzesten Ziffernfolge `s` (k Ziffern).
    // Rusts `{:e}` liefert genau diese kuerzeste Folge — sie selbst zu
    // rechnen hiesse Grisu nachzubauen.
    let neg = n < 0.0;
    let sci = alloc::format!("{:e}", libm::fabs(n));
    let (mant, ex) = match sci.split_once('e') { Some(x) => x, None => return sci };
    let exp: i32 = ex.parse().unwrap_or(0);
    let d: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let d = d.trim_end_matches('0');
    let d = if d.is_empty() { "0" } else { d };
    let k = d.len() as i32;
    let pt = exp + 1;                    // wo das Komma steht
    let mut s = String::new();
    if neg { s.push('-'); }
    if (1..=21).contains(&pt) {
        if k <= pt {
            s.push_str(d);
            for _ in 0..(pt - k) { s.push('0'); }
        } else {
            s.push_str(&d[..pt as usize]);
            s.push('.');
            s.push_str(&d[pt as usize..]);
        }
    } else if (-5..=0).contains(&pt) {
        s.push_str("0.");
        for _ in 0..(-pt) { s.push('0'); }
        s.push_str(d);
    } else {
        s.push_str(&d[..1]);
        if k > 1 { s.push('.'); s.push_str(&d[1..]); }
        s.push('e');
        s.push(if pt - 1 < 0 { '-' } else { '+' });
        s.push_str(&alloc::format!("{}", (pt - 1).abs()));
    }
    s
}

/// `Number.prototype.toString(radix)` fuer eine Basis ausser 10.
///
/// Der Weg ist der von V8 (`DoubleToRadixCString`) und nicht ein eigener:
/// die Spezifikation laesst die Nachkommastellen ausdruecklich offen
/// („implementation-approximated"), und zwei Motoren, die hier verschieden
/// runden, geben verschiedene Farbwerte aus. Byte-gleich zu node
/// gegengeprueft.
pub fn num_to_radix(v: f64, radix: u32) -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if v.is_nan() { return "NaN".to_string(); }
    if v == f64::INFINITY { return "Infinity".to_string(); }
    if v == f64::NEG_INFINITY { return "-Infinity".to_string(); }
    if v == 0.0 { return "0".to_string(); }
    let neg = v < 0.0;
    let value = libm::fabs(v);
    let r = radix as f64;
    let mut integer = libm::floor(value);
    let mut fraction = value - integer;
    // Der Abstand zur naechsten darstellbaren Zahl ist die Abbruchgrenze:
    // weiter zu rechnen hiesse Ziffern zu drucken, die im `f64` nicht stehen.
    let mut delta = 0.5 * (libm::nextafter(value, f64::INFINITY) - value);
    if delta < 5e-324 { delta = 5e-324; }
    let mut frac: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= r;
            delta *= r;
            let digit = f64_to_usize(fraction);
            frac.push(CHARS[digit.min(35)]);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                // Aufrunden mit Uebertrag — und laeuft der bis vor die erste
                // Stelle, waechst die Ganzzahl.
                loop {
                    match frac.pop() {
                        None => { integer += 1.0; break }
                        Some(c) => {
                            let d = CHARS.iter().position(|x| *x == c).unwrap_or(0) as u32;
                            if d + 1 < radix { frac.push(CHARS[(d + 1) as usize]); break }
                        }
                    }
                }
                break;
            }
            if fraction < delta { break }
        }
    }
    // Ueber 2^53 traegt der `f64` die unteren Stellen nicht mehr — dort
    // stehen Nullen, und das ist ehrlicher als erfundene Ziffern.
    let mut int_digits: Vec<u8> = Vec::new();
    while integer / r >= 9007199254740992.0 {
        integer /= r;
        int_digits.push(b'0');
    }
    loop {
        let rem = libm::fmod(integer, r);
        int_digits.push(CHARS[f64_to_usize(rem).min(35)]);
        integer = (integer - rem) / r;
        if integer <= 0.0 { break }
    }
    int_digits.reverse();
    let mut out = String::new();
    if neg { out.push('-'); }
    for c in int_digits { out.push(c as char); }
    if !frac.is_empty() {
        out.push('.');
        for c in frac { out.push(c as char); }
    }
    out
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

/// `f64` zu einer kleinen Ganzzahl, OHNE `i64.trunc_sat_f64_u`.
///
/// **forge kann diesen Befehl nicht**, und ein Modul mit ihm bleibt am Geraet
/// beim ERSTEN Aufruf der Stelle stehen — nicht beim Laden, wo es auffiele.
/// Der Umweg ueber `u32` ist fuer alles, was hier gezaehlt wird (Monate,
/// Ziffern, Schiebeweiten, Indizes), derselbe Wert und uebersetzt.
/// `python3 tools/forge-gate.py` ist der Vorablauf, der das prueft.
pub fn f64_to_usize(v: f64) -> usize {
    if !(v > 0.0) { return 0 }
    if v >= 4294967295.0 { return u32::MAX as usize }
    (v as u32) as usize
}

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


#[cfg(test)]
mod zahltext {
    /// **`Number::toString` hat eigene Regeln, und der Schnellweg muss sie
    /// treffen.** Die Tabelle steht gegen echtes JS, nicht gegen die alte
    /// Fassung: eine Probe, die nur „wie vorher" prueft, zementiert einen
    /// Fehler, statt ihn zu finden.
    #[test]
    fn zahlen_werden_wie_in_js_geschrieben() {
        let f: &[(f64, &str)] = &[
            (0.0, "0"), (-0.0, "0"), (1.0, "1"), (-1.0, "-1"), (42.0, "42"),
            (100.0, "100"), (-7.0, "-7"),
            // Genau an der Grenze des Schnellwegs: 2^53-1 geht darueber,
            // 2^53 faellt auf den langen Weg — beide muessen gleich lauten.
            (9007199254740991.0, "9007199254740991"),
            (9007199254740992.0, "9007199254740992"),
            // Und darueber, wo JS immer noch ausschreibt (pt <= 21).
            (1e20, "100000000000000000000"),
            // Ab 1e21 exponentiell.
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (0.1, "0.1"), (-0.5, "-0.5"), (123.456, "123.456"),
            (0.000001, "0.000001"),
            (f64::INFINITY, "Infinity"), (f64::NEG_INFINITY, "-Infinity"),
        ];
        for (n, want) in f {
            assert_eq!(&super::num_to_string(*n), want, "fuer {n}");
        }
        assert_eq!(&super::num_to_string(f64::NAN), "NaN");
    }
}
