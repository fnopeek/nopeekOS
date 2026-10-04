//! Values and the object model.
//!
//! Properties carry full descriptors (`writable`/`enumerable`/`configurable`)
//! from the start; retrofitting them would touch every access path.
//!
//! Memory is reference-counted (`Rc`), with no cycle collector: cycles leak
//! until the interpreter instance is dropped. `Gc` is where a collector
//! would hook in.

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
    /// Arbitrary-precision integer. A primitive, not an object.
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
                // A bound function is callable; a proxy is callable if its
                // target is.
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

    /// `===`: NaN is never equal, `-0 === 0`.
    pub fn strict_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            // Symbol identity is its key, which is unique per symbol, so
            // identity does not depend on the `Rc` and `Symbol.for` may
            // rebuild the data.
            (Value::Sym(a), Value::Sym(b)) => a.key == b.key,
            (Value::BigInt(a), Value::BigInt(b)) => a == b,
            (Value::Obj(a), Value::Obj(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// SameValue (`Object.is`): like `===`, but NaN equals itself and `-0`
    /// differs from `0`.
    pub fn same_value(&self, other: &Value) -> bool {
        if let (Value::Num(a), Value::Num(b)) = (self, other) {
            if a.is_nan() && b.is_nan() { return true; }
            if *a == 0.0 && *b == 0.0 { return a.is_sign_negative() == b.is_sign_negative(); }
        }
        self.strict_eq(other)
    }
}

/// A property name: string or symbol, in one table.
///
/// A symbol's key is a string with a leading NUL byte. This avoids a second
/// key type, which would force building a `PropName` on every `o.foo` lookup;
/// `get_own(&str)` stays allocation-free.
///
/// Trade-off: a script writing `obj["\0#7"]` reaches the symbol namespace.
/// A leading NUL does not occur in real code, and the check is one byte
/// (`is_sym_key`).
pub type PropName = Rc<str>;

/// A symbol. `key` is the property name it is stored under and also its
/// identity (see `Value::strict_eq`).
pub struct SymData {
    pub desc: Option<Rc<str>>,
    pub key: Rc<str>,
    /// Set by `Symbol.for`; this is what `Symbol.keyFor` returns.
    pub registered: Option<Rc<str>>,
}

/// Is this property name a symbol key?
#[inline]
pub fn is_sym_key(k: &str) -> bool { k.as_bytes().first() == Some(&0) }

/// Is this the key of a private field `#name`?
///
/// Private keys start with NUL, so `own_keys` skips them and the field is
/// invisible to `Object.keys`, `for..in`, `JSON.stringify` and spread
/// without a second store. Symbol keys also start with NUL, so the second
/// byte must differ or `getOwnPropertySymbols` would leak private fields.
/// Markers after the NUL: `@` well-known, `*` registered, `#` ordinary
/// symbol (`Interp::new_symbol`), `~` private.
///
/// Limitation: two classes that both define `#p` on the same object share
/// it; real engines key private names per class.
pub fn is_private_key(k: &str) -> bool { k.as_bytes().starts_with(b"\0~") }

pub fn private_key(name: &str) -> Rc<str> {
    Rc::from(alloc::format!("{PRIVATE_PREFIX}{name}").as_str())
}

/// Key prefix of a private field. Scripts cannot produce it (NUL is not a
/// valid identifier character), so a prefix match is a safe brand check.
pub const PRIVATE_PREFIX: &str = "\0~";

/// The field name without the prefix, for error messages.
pub fn private_name(key: &str) -> &str {
    key.strip_prefix(PRIVATE_PREFIX).unwrap_or(key)
}

/// Rebuild a symbol from its key.
///
/// `Object.getOwnPropertySymbols` must return symbols, but the table only
/// holds keys, so the key encodes description and registration:
///
///   `\0@iterator`  well-known   -> `Symbol.iterator`
///   `\0*name`      registered   -> `Symbol.for("name")`
///   `\0#7`         anonymous    -> `Symbol()`
///   `\0#7:text`    described    -> `Symbol("text")`
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

/// Keys of the well-known symbols, as constants so builtins can write
/// `self.get(v, SYM_ITERATOR)` without looking up the symbol object.
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

/// Internal state of a builtin iterator. NUL-prefixed, hence hidden from
/// `own_keys` and scripts; `native` takes no closure, so state lives on the
/// object.
pub const IT_TARGET: &str = "\0!target";
/// Stands in for the internal slot `[[SetData]]`/`[[MapData]]`: which
/// collection this object is. Hidden from scripts; without it a plain object
/// would pass the brand check of e.g. `Set.prototype.union`.
pub const COLL_KIND: &str = "\0!coll";
pub const EV_REASON: &str = "\0!ev.reason";
pub const EV_PROMISE: &str = "\0!ev.promise";
pub const IT_INDEX: &str = "\0!index";
/// 0 = values, 1 = keys, 2 = entries.
pub const IT_KIND: &str = "\0!kind";

/// Source list for `Symbol.iterator` etc. on the global object.
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

/// A partial property descriptor, as passed to `Object.defineProperty`.
///
/// Unlike `Prop` (a stored property with every field set), a missing field
/// here means "leave as is", not `false`.
///
/// `Some(Value::Undefined)` means "present and `undefined`" and differs from
/// `None`: `{get: undefined}` makes an accessor property without a getter.
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
    /// Neither accessor nor data, e.g. `{enumerable: true}` alone.
    pub fn is_generic(&self) -> bool { !self.is_accessor() && !self.is_data() }
    pub fn is_empty(&self) -> bool {
        self.is_generic() && self.enumerable.is_none() && self.configurable.is_none()
    }
    /// The descriptor of an existing property: every field present.
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
    /// The property created from this descriptor: missing fields take their
    /// defaults (`false`/`undefined`). Only for creation, not for redefinition.
    pub fn into_new_prop(self) -> Prop {
        Prop {
            value: if self.is_accessor() { None }
                   else { Some(self.value.unwrap_or(Value::Undefined)) },
            // Keep `undefined`: `{set: undefined}` is an accessor property
            // without a setter. Filtering it to `None` would turn it into a
            // data property and wrongly reject a later redefinition.
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
    /// Attributes of builtin properties: writable and configurable, but not
    /// enumerable (ES 17), so `for..in` on a fresh object does not see
    /// `toString`.
    pub fn builtin(v: Value) -> Prop {
        Prop { value: Some(v), get: None, set: None, writable: true, enumerable: false, configurable: true }
    }
    /// Attributes of `@@toStringTag` (and `Symbol.prototype[@@toPrimitive]`):
    /// non-writable, non-enumerable, but configurable (ES 17).
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
    /// May be called with `new`.
    pub ctor: bool,
}

pub struct FuncData {
    pub node: Rc<crate::js::ast::Func>,
    pub env: Rc<RefCell<crate::js::interp::Env>>,
    /// Bound `this`: arrow functions capture it, ordinary functions receive it
    /// at call time.
    pub this_val: Option<Value>,
    pub home_object: Option<Gc>,
    /// The class whose constructor this is; set only if the class has
    /// instance fields.
    ///
    /// Instance fields belong neither on the prototype nor in the body; they
    /// are initialized at construction, which needs this list. `env` is
    /// already the right scope for the initializers.
    pub class: Option<Rc<crate::js::ast::Class>>,
}

/// The byte storage behind every TypedArray and DataView.
///
/// An `ArrayBuffer` is the storage, a TypedArray only a view onto it. Two
/// views on the same buffer see each other's writes, hence the bytes live
/// here and not in the view.
pub struct BufData {
    pub bytes: RefCell<alloc::vec::Vec<u8>>,
    /// Detached: the buffer then has length 0 and every view on it is empty.
    /// test262 triggers this through `$262.detachArrayBuffer`.
    pub detached: core::cell::Cell<bool>,
}

/// Element type of a view.
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
    /// Does this type hold BigInts? Then writers must use `ToBigInt` instead
    /// of `ToNumber`.
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

    /// Read an element as a JS value; a BigInt for the 64-bit types.
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

    /// Write a BigInt, truncated to 64 bits.
    pub fn write_big(self, b: &mut [u8], at: usize, v: &crate::js::bigint::Big) {
        let raw = v.to_u64_wrap();
        for k in 0..8 { b[at + k] = ((raw >> (8 * k)) & 0xff) as u8; }
    }
    /// Read an element. Always little-endian: unlike `DataView`, the spec
    /// gives typed arrays no choice, and every target platform is LE.
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
            // Read via `read_v`; listed here only for exhaustiveness.
            ElemKind::I64 => g(8) as i64 as f64,
            ElemKind::U64 => g(8) as f64,
        }
    }
    /// Write an element with the spec's conversion: integer types map NaN
    /// and infinities to 0 and wrap modulo 2^n; `Uint8Clamped` clamps and
    /// rounds.
    pub fn write(self, b: &mut [u8], at: usize, v: f64) {
        let put = |b: &mut [u8], n: usize, raw: u64| {
            for k in 0..n { b[at + k] = ((raw >> (8 * k)) & 0xff) as u8; }
        };
        match self {
            ElemKind::U8C => {
                b[at] = if v.is_nan() { 0 } else if v <= 0.0 { 0 } else if v >= 255.0 { 255 }
                        else {
                            // Round half to even, as the spec requires;
                            // `round` would round half away from zero.
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

/// `ToIntegerOrInfinity` followed by modulo 2^bits: the shared core of
/// `ToInt8`/`ToUint8`/`ToInt16`/...
pub fn to_uint_wrap(v: f64, bits: usize) -> u64 {
    // Go through `to_uint32` rather than a direct f64->u64 cast: that cast
    // compiles to `i64.trunc_sat_f64_u`, which forge does not support, and
    // the failure would only surface when the code path first runs.
    // `ToUint32` already truncates and wraps modulo 2^32; smaller widths are
    // a mask of it.
    let full = to_uint32(v) as u64;
    if bits >= 32 { full } else { full & ((1u64 << bits) - 1) }
}

/// A view onto a buffer.
pub struct TaData {
    pub buf: Gc,
    pub kind: ElemKind,
    /// Byte offset into the buffer.
    pub offset: usize,
    /// Number of elements, not bytes.
    pub len: usize,
}

/// The untyped view: every access names its own type and byte order, so
/// there is no `ElemKind` here.
pub struct DvData {
    pub buf: Gc,
    pub offset: usize,
    pub len: usize,
}

impl TaData {
    /// The actual element count: a detached or shrunk buffer makes the view
    /// empty without the view being touched.
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
    /// Bound function from `Function.prototype.bind`.
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
    /// A module namespace object, with the module's URL.
    ///
    /// An exotic object (ES2024 §10.4.6): its properties are live bindings,
    /// not values, so `ns.x` reflects later assignments to an exported `let`.
    /// The property table is still filled; it answers `Object.keys(ns)` and
    /// `for..in`.
    ModuleNs(Rc<str>),
    /// `el.dataset`; the number is the owning node.
    ///
    /// A separate kind because writes must reach through to the `data-*`
    /// attribute; an ordinary property would leave the attribute unchanged.
    Dataset(u32),
    /// The storage (`ArrayBuffer`) and the two kinds of view onto it.
    Buffer(Rc<BufData>),
    TypedArray(Rc<TaData>),
    DataView(Rc<DvData>),
    /// A proxy: target and handler, or `None` once revoked. The traps live
    /// in the fundamental operations, see `proxy.rs`.
    Proxy(crate::js::proxy::ProxyCell),
    /// The time value of a `Date`. A separate kind rather than a property, so
    /// it does not appear in `Object.getOwnPropertyNames`.
    Date(Rc<core::cell::Cell<f64>>),
    /// A suspended generator: its own machine with its state. See
    /// `generator.rs`, and the header of `vm.rs` for why it is a separate
    /// machine rather than a frame in the caller's.
    Generator(Rc<crate::js::generator::GenState>),
    /// Contents of a `Map`/`Set`/`WeakMap`/`WeakSet`.
    Collection(Rc<RefCell<CollData>>),
}

/// A collection key, compared by SameValueZero.
///
/// A dedicated representation rather than a stringified property name:
/// distinct objects must be distinct keys, `1` and `"1"` must differ, and a
/// lookup must not call the key's `toString`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CollKey {
    Undefined,
    Null,
    Bool(bool),
    /// The number's bits, with `-0` mapped to `+0` and every NaN to one
    /// pattern. That is the only difference between SameValueZero and
    /// `Object.is`.
    Num(u64),
    Str(Rc<str>),
    Sym(Rc<str>),
    Big(String),
    /// The object's address, not its contents. It stays valid because
    /// `entries` holds the key itself, keeping it alive.
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

/// The entries of a collection.
///
/// `WeakMap` holds its keys strongly, like `Map`. True weak references need
/// a collector, which does not exist (see the module header); a key that
/// silently vanished would be worse than one that lives too long.
#[derive(Default)]
pub struct CollData {
    /// Entries in insertion order. Deleted entries become `None` instead of
    /// being removed: `forEach` and the iterators walk by index, and
    /// compaction would shift entries under a running iteration, which the
    /// spec defines explicitly.
    pub entries: Vec<Option<(Value, Value)>>,
    /// Key -> index into `entries`, so `get` is not a linear scan.
    pub index: HashMap<CollKey, usize>,
}

impl CollData {
    pub fn get(&self, k: &Value) -> Option<Value> {
        let i = *self.index.get(&CollKey::of(k))?;
        self.entries.get(i).and_then(|e| e.as_ref()).map(|(_, v)| v.clone())
    }
    pub fn has(&self, k: &Value) -> bool { self.index.contains_key(&CollKey::of(k)) }
    /// Insert or update. An existing key keeps its position; order follows
    /// first insertion.
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
    /// Snapshot of the live pairs, from which `keys`, `values` and `entries`
    /// build their iterators.
    pub fn pairs(&self) -> Vec<(Value, Value)> {
        self.entries.iter().flatten().cloned().collect()
    }
}

/// Number of `Object`s currently alive.
///
/// Compared with what a mark pass from the roots can reach, this shows how
/// much lives in `Rc` cycles. Only with `--features heap-census`, so the
/// shipped module carries no per-object counter.
#[cfg(feature = "heap-census")]
pub static mut LIVE_OBJECTS: usize = 0;

#[inline(always)]
fn census_born() {
    #[cfg(feature = "heap-census")]
    // SAFETY: single-threaded wasm module; nothing else accesses the counter
    // concurrently.
    unsafe { LIVE_OBJECTS += 1 }
}

#[cfg(feature = "heap-census")]
impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: single-threaded wasm module; see `census_born`.
        unsafe { LIVE_OBJECTS = LIVE_OBJECTS.saturating_sub(1) }
    }
}

pub struct Object {
    props: HashMap<PropName, Prop>,
    /// Insertion order. JS enumerates properties in a fixed order (integer
    /// keys ascending first, then the rest in insertion order), which a hash
    /// table alone cannot provide.
    order: Vec<PropName>,
    pub proto: Option<Gc>,
    pub kind: ObjKind,
    pub extensible: bool,
}

impl Object {
    pub fn new(proto: Option<Gc>) -> Object {
        census_born();
        Object { props: HashMap::new(), order: Vec::new(), proto, kind: ObjKind::Plain, extensible: true }
    }
    pub fn with_kind(proto: Option<Gc>, kind: ObjKind) -> Object {
        census_born();
        Object { props: HashMap::new(), order: Vec::new(), proto, kind, extensible: true }
    }

    pub fn get_own(&self, k: &str) -> Option<&Prop> { self.props.get(k) }

    /// Is this own key enumerable?
    ///
    /// The answer is not always in the table: a TypedArray's indices are
    /// enumerable without having entries, since they derive from its length.
    /// Ask here instead of `get_own(k).map(|p| p.enumerable)`.
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

    /// Remove every property. Only for tearing down a realm; see
    /// `Interp::teardown` for why that is needed.
    pub fn clear_props(&mut self) {
        self.props.clear();
        self.order.clear();
    }

    /// Remove all index properties in one pass. Calling `remove` per key
    /// would rescan the order list each time, which is O(n^2).
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

    /// Every own key in insertion order, including NUL-prefixed internal
    /// ones. `own_keys` omits those deliberately; code that takes over an
    /// object (`super()` into a builtin constructor) needs them too.
    pub fn raw_keys(&self) -> Vec<PropName> { self.order.clone() }

    /// Own string keys in spec order: integer indices ascending, then the
    /// rest in insertion order. Symbols are excluded, which is what every
    /// caller (`Object.keys`, `for..in`, `JSON.stringify`,
    /// `getOwnPropertyNames`) wants; use `own_sym_keys` for symbols.
    pub fn own_keys(&self) -> Vec<PropName> {
        // A TypedArray's indices are not in the table; they derive from its
        // length.
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

    /// Own symbol keys, in insertion order.
    pub fn own_sym_keys(&self) -> Vec<PropName> {
        self.order.iter().filter(|k| is_sym_key(k) && !is_private_key(k)).cloned().collect()
    }

    pub fn prop_count(&self) -> usize { self.props.len() }
}

/// Is `k` an array index? Only a canonical decimal without a leading zero,
/// below 2^32-1. `"01"` and `"1.0"` are not indices, which affects key order.
pub fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || k.len() > 10 { return None; }
    if k.len() > 1 && k.starts_with('0') { return None; }
    if !k.bytes().all(|b| b.is_ascii_digit()) { return None; }
    k.parse::<u32>().ok().filter(|v| *v < u32::MAX)
}

/// Number to string by JS rules (Number::toString).
///
/// Not `format!("{}")`: Rust writes `1e21` as `1000000000000000000000` and
/// infinity as `inf`.
pub fn num_to_string(n: f64) -> String {
    if n.is_nan() { return "NaN".to_string(); }
    if n == f64::INFINITY { return "Infinity".to_string(); }
    if n == f64::NEG_INFINITY { return "-Infinity".to_string(); }
    if n == 0.0 { return "0".to_string(); }
    // Fast path for integers (indices, lengths, counters), the common case:
    // the long path below allocates via `format!("{:e}")`, and every number
    // turned into a property key would take it.
    //
    // Below 2^53 integers are exact in `f64`, and below 1e21 JS writes them
    // out in full (ES 6.1.6.1.20, case `1 <= pt <= 21`), so this path agrees
    // with the general one.
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
    // ES 6.1.6.1.20 picks the form by the decimal point position:
    // `s * 10^(n-k)` with the shortest digit string `s` (k digits). Rust's
    // `{:e}` yields exactly that shortest string.
    let neg = n < 0.0;
    let sci = alloc::format!("{:e}", libm::fabs(n));
    let (mant, ex) = match sci.split_once('e') { Some(x) => x, None => return sci };
    let exp: i32 = ex.parse().unwrap_or(0);
    let d: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let d = d.trim_end_matches('0');
    let d = if d.is_empty() { "0" } else { d };
    let k = d.len() as i32;
    let pt = exp + 1;                    // position of the decimal point
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

/// `Number.prototype.toString(radix)` for a radix other than 10.
///
/// Follows V8's `DoubleToRadixCString`: the spec leaves the fraction digits
/// implementation-approximated, and matching V8 keeps outputs (e.g. colour
/// values) identical across engines.
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
    // Half the gap to the next representable number is the cut-off: digits
    // beyond it are not present in the `f64`.
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
                // Round up with carry; if it runs past the first fraction
                // digit, the integer part grows.
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
    // Above 2^53 the `f64` no longer holds the low digits; emit zeros
    // there rather than invented digits.
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

/// String to number (`Number("...")`): surrounding whitespace,
/// `0x`/`0o`/`0b`, `Infinity`, and empty = 0.
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

/// `ToInt32`, the conversion behind the bitwise operators: signed modulo
/// 2^32; NaN and infinities become 0.
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 { return 0; }
    let m = libm::trunc(n) % 4294967296.0;
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    if m >= 2147483648.0 { (m - 4294967296.0) as i32 } else { m as i32 }
}
pub fn to_uint32(n: f64) -> u32 { to_int32(n) as u32 }

/// `f64` to a small integer without `i64.trunc_sat_f64_u`.
///
/// forge does not support that instruction, and a module using it fails only
/// when the code path first runs. Going through `u32` gives the same value
/// for everything counted here (months, digits, shift widths, indices).
/// `python3 tools/forge-gate.py` checks for the instruction.
pub fn f64_to_usize(v: f64) -> usize {
    if !(v > 0.0) { return 0 }
    if v >= 4294967295.0 { return u32::MAX as usize }
    (v as u32) as usize
}

/// `ToInteger`: truncate, NaN to 0.
pub fn to_integer(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else if n.is_infinite() { n } else { libm::trunc(n) }
}

pub fn new_obj(proto: Option<Gc>) -> Gc { register(Rc::new(RefCell::new(Object::new(proto)))) }
pub fn new_kind(proto: Option<Gc>, kind: ObjKind) -> Gc {
    register(Rc::new(RefCell::new(Object::with_kind(proto, kind))))
}

/// Every object ever built, as a weak reference.
///
/// A sweeping collector needs to enumerate all objects, which `Rc` cannot.
/// The list holds nothing alive (`Weak`). Only with
/// `--features heap-census`.
#[cfg(feature = "heap-census")]
pub static mut ALL_OBJECTS: alloc::vec::Vec<alloc::rc::Weak<RefCell<Object>>> =
    alloc::vec::Vec::new();

/// List length at which the registry is compacted next.
#[cfg(feature = "heap-census")]
static mut NEXT_COMPACT: usize = 1024;

#[inline(always)]
fn register(g: Gc) -> Gc {
    #[cfg(feature = "heap-census")]
    // SAFETY: single-threaded wasm module; nothing else holds a reference
    // to `ALL_OBJECTS` or `NEXT_COMPACT` while this runs.
    unsafe {
        let all = &mut *(&raw mut ALL_OBJECTS);
        // Compact at a doubling threshold rather than a ratio: a fixed ratio
        // would rescan the O(n) list almost every allocation on a page that
        // produces garbage constantly, i.e. O(n^2).
        if all.len() >= NEXT_COMPACT {
            all.retain(|w| w.strong_count() > 0);
            NEXT_COMPACT = (all.len() * 2).max(1024);
        }
        all.push(alloc::rc::Rc::downgrade(&g));
    }
    g
}

/// Build a native function object (avoids a `Box<dyn ...>`).
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
    /// `Number::toString` has its own rules, and the fast path must match
    /// them. The expected values come from real JS.
    #[test]
    fn zahlen_werden_wie_in_js_geschrieben() {
        let f: &[(f64, &str)] = &[
            (0.0, "0"), (-0.0, "0"), (1.0, "1"), (-1.0, "-1"), (42.0, "42"),
            (100.0, "100"), (-7.0, "-7"),
            // At the fast path boundary: 2^53-1 takes it, 2^53 takes the
            // long path; both must print the same way.
            (9007199254740991.0, "9007199254740991"),
            (9007199254740992.0, "9007199254740992"),
            // Above that JS still writes digits out (pt <= 21).
            (1e20, "100000000000000000000"),
            // From 1e21 on, exponential.
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
