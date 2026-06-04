//! AML runtime values and name paths.

use alloc::{rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

/// A 4-byte ACPI NameSeg (trailing '_' padded).
pub type Seg = [u8; 4];
/// Absolute namespace path = chain of segments from the root.
pub type Path = Vec<Seg>;
/// A mutable data-object cell (Name value, package element, Local/Arg slot).
pub type Obj = Rc<RefCell<Value>>;

pub fn obj(v: Value) -> Obj {
    Rc::new(RefCell::new(v))
}

#[derive(Clone)]
pub enum Value {
    Uninit,
    Int(u64),
    Str(String),
    Buffer(Vec<u8>),
    Package(Vec<Obj>),
    /// A reference to a writable place produced by Index / RefOf / a bare name
    /// used as a Store target.
    Ref(Place),
}

#[derive(Clone)]
pub enum Place {
    /// A plain data-object cell.
    Obj(Obj),
    /// A field unit at an absolute namespace path (read/write hits its region).
    Field(Path),
    /// A byte inside a Buffer object.
    BufIndex(Obj, usize),
}

impl Value {
    pub fn as_int(&self) -> u64 {
        match self {
            Value::Int(n) => *n,
            Value::Uninit => 0,
            Value::Buffer(b) => {
                // First up-to-8 bytes, little-endian.
                let mut n = 0u64;
                for (i, &x) in b.iter().take(8).enumerate() {
                    n |= (x as u64) << (i * 8);
                }
                n
            }
            _ => 0,
        }
    }
}

pub fn seg(s: &str) -> Seg {
    let b = s.as_bytes();
    let mut out = [b'_'; 4];
    for i in 0..4.min(b.len()) {
        out[i] = b[i];
    }
    out
}

/// Render a path like `\_SB.PCI0.LPCB.EC0_.BRC_` for logging.
pub fn path_str(p: &Path) -> String {
    let mut s = String::from("\\");
    for (i, sg) in p.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        for &c in sg {
            s.push(c as char);
        }
    }
    s
}
