//! AML method evaluator — enough opcodes to run the firmware's battery methods.

use crate::value::{obj, path_str, Obj, Path, Place, Seg, Value};
use crate::{Ec, Namespace, Node};
use alloc::collections::BTreeMap;
use alloc::{format, string::String, vec, vec::Vec};

const MAX_DEPTH: usize = 64;

pub struct Interp<'a> {
    ns: &'a Namespace,
    ec: &'a mut dyn Ec,
    depth: usize,
    /// In-memory backing for non-EmbeddedControl regions (SystemIO,
    /// SystemMemory, PCI config, ...). Keyed by (region_space, absolute_byte).
    /// Only EmbeddedControl touches real hardware; the firmware's SMI/init
    /// handshakes thus become harmless writes-readable-back here, so their
    /// write-then-poll loops terminate without any real port I/O.
    mem: BTreeMap<(u8, u64), u8>,
}

struct Frame<'a> {
    scope: Path,
    args: Vec<Obj>,
    locals: Vec<Obj>,
    body: &'a [u8],
}

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

type R<T> = Result<T, String>;

// ── public entry points ───────────────────────────────────────────────

pub fn find_batteries(ns: &Namespace) -> Vec<Path> {
    let mut out = Vec::new();
    for (path, node) in ns.nodes.iter() {
        if let Node::Name(v) = node {
            if path.last() == Some(&crate::value::seg("_HID")) {
                if is_pnp0c0a(&v.borrow()) {
                    let mut dev = path.clone();
                    dev.pop(); // drop _HID -> the device path
                    out.push(dev);
                }
            }
        }
    }
    out
}

fn is_pnp0c0a(v: &Value) -> bool {
    // _HID may be an EisaId-encoded integer or a string "PNP0C0A".
    match v {
        Value::Str(s) => s == "PNP0C0A",
        Value::Int(n) => *n == eisa_id("PNP0C0A"),
        _ => false,
    }
}

/// EisaId packing (ACPI): 7-bit compressed mfg + hex product, big-endian dword.
fn eisa_id(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() != 7 {
        return 0;
    }
    let c = |x: u8| -> u64 {
        if x.is_ascii_digit() { (x - b'0') as u64 } else { (x - b'A' + 10) as u64 }
    };
    let m0 = (b[0] - b'@') as u64;
    let m1 = (b[1] - b'@') as u64;
    let m2 = (b[2] - b'@') as u64;
    let prod = (c(b[3]) << 12) | (c(b[4]) << 8) | (c(b[5]) << 4) | c(b[6]);
    let swapped = (m0 << 26) | (m1 << 21) | (m2 << 16) | prod;
    // Stored little-endian in the dword -> byte-swap for comparison.
    ((swapped >> 24) & 0xFF)
        | (((swapped >> 16) & 0xFF) << 8)
        | (((swapped >> 8) & 0xFF) << 16)
        | ((swapped & 0xFF) << 24)
}

pub fn read_battery(ns: &Namespace, ec: &mut dyn Ec, bat: &Path) -> R<crate::BatteryInfo> {
    let mut it = Interp { ns, ec, depth: 0, mem: BTreeMap::new() };
    it.register_ec_regions()?;

    // _BST -> Package { State, PresentRate, RemainingCapacity, Voltage }
    let mut p = bat.clone();
    p.push(crate::value::seg("_BST"));
    let bst = it.call_path(&p, Vec::new())?;
    let (state, remaining) = match &bst {
        Value::Package(e) if e.len() >= 4 => {
            (e[0].borrow().as_int() as u32, e[2].borrow().as_int() as u32)
        }
        _ => return Err(format!("_BST did not return a Package(>=4): got {}", kind(&bst))),
    };

    // Absent batteries report 0xFFFFFFFF in every field.
    if remaining == 0xFFFF_FFFF {
        return Ok(crate::BatteryInfo { present: false, ..Default::default() });
    }

    // _BIF / _BIX -> full charge capacity.
    let full = it.read_full_charge(bat)?;

    let percent = if full > 0 {
        (((remaining as u64) * 100 + (full as u64) / 2) / full as u64).min(100) as u8
    } else {
        0
    };

    Ok(crate::BatteryInfo {
        present: true,
        state,
        remaining_mah: remaining,
        full_charge_mah: full,
        percent,
    })
}

impl<'a> Interp<'a> {
    fn read_full_charge(&mut self, bat: &Path) -> R<u32> {
        // _BIF: Package[1]=DesignCap, [2]=LastFullChargeCap.
        let mut p = bat.clone();
        p.push(crate::value::seg("_BIF"));
        if self.ns.nodes.contains_key(&p) {
            let v = self.call_path(&p, Vec::new())?;
            if let Value::Package(e) = &v {
                if e.len() >= 3 {
                    return Ok(e[2].borrow().as_int() as u32);
                }
            }
        }
        // _BIX: Package[2]=DesignCap, [3]=LastFullChargeCap (Revision at [0]).
        let mut p = bat.clone();
        p.push(crate::value::seg("_BIX"));
        if self.ns.nodes.contains_key(&p) {
            let v = self.call_path(&p, Vec::new())?;
            if let Value::Package(e) = &v {
                if e.len() >= 4 {
                    return Ok(e[3].borrow().as_int() as u32);
                }
            }
        }
        Err(String::from("neither _BIF nor _BIX usable"))
    }

    /// Run every EmbeddedControl region's parent `_REG(3, 1)` so the firmware
    /// sets its "EC ready" gate (e.g. ECRG = 1). Generic — no name hardcoded.
    fn register_ec_regions(&mut self) -> R<()> {
        let mut parents: Vec<Path> = Vec::new();
        for (path, node) in self.ns.nodes.iter() {
            if let Node::Region { space: 3, .. } = node {
                let mut par = path.clone();
                par.pop();
                if !parents.contains(&par) {
                    parents.push(par);
                }
            }
        }
        for par in parents {
            let mut reg = par.clone();
            reg.push(crate::value::seg("_REG"));
            if self.ns.nodes.contains_key(&reg) {
                let args = vec![obj(Value::Int(3)), obj(Value::Int(1))];
                let _ = self.call_path(&reg, args)?;
            }
        }
        Ok(())
    }

    fn call_path(&mut self, path: &Path, args: Vec<Obj>) -> R<Value> {
        if self.depth > MAX_DEPTH {
            return Err(String::from("recursion too deep"));
        }
        let (body, scope) = match self.ns.get(path) {
            Some(Node::Method { body, scope, .. }) => (body.as_slice(), scope.clone()),
            Some(Node::Name(v)) => return Ok(v.borrow().clone()),
            Some(Node::Field { .. }) => return self.read_field(path),
            _ => return Err(format!("call: {} is not a method", path_str(path))),
        };
        // The method's own scope is the path itself (names it creates live here);
        // unqualified lookups search upward from here.
        let mut locals = Vec::with_capacity(8);
        for _ in 0..8 {
            locals.push(obj(Value::Uninit));
        }
        let frame = Frame { scope: path.clone(), args, locals, body };
        let _ = scope; // body uses `path` as its scope anchor
        self.depth += 1;
        let r = self.exec_list(&frame, 0, frame.body.len());
        self.depth -= 1;
        match r {
            Ok(Flow::Return(v)) => Ok(v),
            Ok(_) => Ok(Value::Uninit),
            Err(e) => Err(format!("{} -> {}", path_str(path), e)),
        }
    }

    // ── statement execution ───────────────────────────────────────────

    fn exec_list(&mut self, f: &Frame, start: usize, end: usize) -> R<Flow> {
        let mut p = start;
        while p < end {
            let (flow, np) = self.stmt(f, p, end)?;
            match flow {
                Flow::Normal => p = np,
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn stmt(&mut self, f: &Frame, p: usize, _end: usize) -> R<(Flow, usize)> {
        let b = f.body;
        match b[p] {
            0xA0 => {
                // If
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let (cond, p2) = self.eval(f, p1)?;
                if cond.as_int() != 0 {
                    let flow = self.exec_list(f, p2, pkg_end)?;
                    if !matches!(flow, Flow::Normal) {
                        return Ok((flow, pkg_end));
                    }
                    // fall through past a possible Else
                    return Ok((Flow::Normal, skip_else(b, pkg_end)));
                } else {
                    // skip then-block; run Else if present
                    if pkg_end < b.len() && b[pkg_end] == 0xA1 {
                        let (else_end, e1) = pkg_length(b, pkg_end + 1);
                        let flow = self.exec_list(f, e1, else_end)?;
                        return Ok((flow, else_end));
                    }
                    return Ok((Flow::Normal, pkg_end));
                }
            }
            0xA1 => {
                // Stray Else (then-branch was taken and consumed it via skip_else,
                // so reaching here means skip it).
                let (else_end, _e1) = pkg_length(b, p + 1);
                Ok((Flow::Normal, else_end))
            }
            0xA2 => {
                // While
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let mut guard = 0u32;
                loop {
                    let (cond, p2) = self.eval(f, p1)?;
                    if cond.as_int() == 0 {
                        break;
                    }
                    match self.exec_list(f, p2, pkg_end)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok((Flow::Return(v), pkg_end)),
                        _ => {}
                    }
                    guard += 1;
                    if guard > 1_000_000 {
                        return Err(String::from("While loop runaway"));
                    }
                }
                Ok((Flow::Normal, pkg_end))
            }
            0xA3 => Ok((Flow::Normal, p + 1)), // Noop
            0xA4 => {
                // Return TermArg
                let (v, np) = self.eval(f, p + 1)?;
                Ok((Flow::Return(v), np))
            }
            0xA5 => Ok((Flow::Break, p + 1)),
            0x9F => Ok((Flow::Continue, p + 1)),
            _ => {
                // Expression statement (Store, method call, op with target...).
                let (_v, np) = self.eval(f, p)?;
                Ok((Flow::Normal, np))
            }
        }
    }

    // ── expression evaluation ─────────────────────────────────────────

    fn eval(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x00 => Ok((Value::Int(0), p + 1)),
            0x01 => Ok((Value::Int(1), p + 1)),
            0xFF => Ok((Value::Int(u64::MAX), p + 1)),
            0x0A => Ok((Value::Int(b[p + 1] as u64), p + 2)),
            0x0B => Ok((Value::Int(u16::from_le_bytes([b[p + 1], b[p + 2]]) as u64), p + 3)),
            0x0C => Ok((
                Value::Int(u32::from_le_bytes([b[p + 1], b[p + 2], b[p + 3], b[p + 4]]) as u64),
                p + 5,
            )),
            0x0E => {
                let mut a = [0u8; 8];
                a.copy_from_slice(&b[p + 1..p + 9]);
                Ok((Value::Int(u64::from_le_bytes(a)), p + 9))
            }
            0x0D => {
                let mut q = p + 1;
                let mut s = String::new();
                while q < b.len() && b[q] != 0 {
                    s.push(b[q] as char);
                    q += 1;
                }
                Ok((Value::Str(s), q + 1))
            }
            0x11 => {
                // Buffer
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let (size, p2) = self.eval(f, p1)?;
                let n = size.as_int() as usize;
                let mut buf = Vec::with_capacity(n);
                buf.extend_from_slice(&b[p2..pkg_end]);
                buf.resize(n, 0);
                Ok((Value::Buffer(buf), pkg_end))
            }
            0x12 => {
                // Package
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let num = b[p1] as usize;
                let mut q = p1 + 1;
                let mut elems = Vec::with_capacity(num);
                while q < pkg_end && elems.len() < num {
                    let (e, nq) = self.eval(f, q)?;
                    elems.push(obj(e));
                    q = nq;
                }
                while elems.len() < num {
                    elems.push(obj(Value::Uninit));
                }
                Ok((Value::Package(elems), pkg_end))
            }
            0x60..=0x67 => Ok((f.locals[(op - 0x60) as usize].borrow().clone(), p + 1)),
            0x68..=0x6E => {
                let i = (op - 0x68) as usize;
                let v = if i < f.args.len() { f.args[i].borrow().clone() } else { Value::Uninit };
                Ok((v, p + 1))
            }
            _ => self.eval_op(f, p),
        }
    }

    fn eval_op(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x70 => {
                // Store(src, SuperName)
                let (src, p1) = self.eval(f, p + 1)?;
                let (place, p2) = self.super_name(f, p1)?;
                if let Some(pl) = place {
                    self.store(&pl, src.clone())?;
                }
                Ok((src, p2))
            }
            0x71 => {
                // RefOf(SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let v = match place {
                    Some(Place::Obj(o)) => Value::Ref(Place::Obj(o)),
                    Some(pl) => Value::Ref(pl),
                    None => Value::Uninit,
                };
                Ok((v, p1))
            }
            0x72 | 0x74 | 0x77 | 0x79 | 0x7A | 0x7B | 0x7C | 0x7D | 0x7E | 0x7F => {
                // Binary op: a, b, target
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (tgt, p3) = self.super_name(f, p2)?;
                let x = a.as_int();
                let y = bb.as_int();
                let r = match op {
                    0x72 => x.wrapping_add(y),
                    0x74 => x.wrapping_sub(y),
                    0x77 => x.wrapping_mul(y),
                    0x79 => if y >= 64 { 0 } else { x << y },
                    0x7A => if y >= 64 { 0 } else { x >> y },
                    0x7B => x & y,
                    0x7C => !(x & y),
                    0x7D => x | y,
                    0x7E => !(x | y),
                    0x7F => x ^ y,
                    _ => unreachable!(),
                };
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p3))
            }
            0x78 => {
                // Divide(a, b, remainder_target, quotient_target) -> quotient
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (rem_t, p3) = self.super_name(f, p2)?;
                let (quo_t, p4) = self.super_name(f, p3)?;
                let x = a.as_int();
                let y = bb.as_int();
                if y == 0 {
                    return Err(String::from("Divide by zero"));
                }
                let q = x / y;
                let r = x % y;
                if let Some(pl) = rem_t {
                    self.store(&pl, Value::Int(r))?;
                }
                if let Some(pl) = quo_t {
                    self.store(&pl, Value::Int(q))?;
                }
                Ok((Value::Int(q), p4))
            }
            0x80 => {
                // Not(operand, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = !a.as_int();
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x75 | 0x76 => {
                // Increment / Decrement (SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let pl = place.ok_or_else(|| String::from("Incr/Decr needs target"))?;
                let cur = self.read_place(&pl)?.as_int();
                let r = if op == 0x75 { cur.wrapping_add(1) } else { cur.wrapping_sub(1) };
                self.store(&pl, Value::Int(r))?;
                Ok((Value::Int(r), p1))
            }
            0x90 | 0x91 => {
                // LAnd / LOr
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let r = if op == 0x90 {
                    (a.as_int() != 0) && (bb.as_int() != 0)
                } else {
                    (a.as_int() != 0) || (bb.as_int() != 0)
                };
                Ok((Value::Int(r as u64), p2))
            }
            0x92 => {
                // LNot, or combined LNotEqual/LLessEqual/LGreaterEqual
                let nb = b[p + 1];
                match nb {
                    0x93 | 0x94 | 0x95 => {
                        let (a, p1) = self.eval(f, p + 2)?;
                        let (bb, p2) = self.eval(f, p1)?;
                        let (x, y) = (a.as_int(), bb.as_int());
                        let r = match nb {
                            0x93 => x != y,      // LNotEqual
                            0x94 => !(x > y),    // LLessEqual
                            0x95 => !(x < y),    // LGreaterEqual
                            _ => unreachable!(),
                        };
                        Ok((Value::Int(r as u64), p2))
                    }
                    _ => {
                        let (a, p1) = self.eval(f, p + 1)?;
                        Ok((Value::Int((a.as_int() == 0) as u64), p1))
                    }
                }
            }
            0x93 | 0x94 | 0x95 => {
                // LEqual / LGreater / LLess
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (x, y) = (a.as_int(), bb.as_int());
                let r = match op {
                    0x93 => x == y,
                    0x94 => x > y,
                    0x95 => x < y,
                    _ => unreachable!(),
                };
                Ok((Value::Int(r as u64), p2))
            }
            0x88 => {
                // Index(source, index, target?) -> reference
                let (place, p1) = self.index_place(f, p + 1)?;
                // optional target
                let (tgt, p2) = self.super_name(f, p1)?;
                let refv = Value::Ref(place.clone());
                if let Some(pl) = tgt {
                    self.store(&pl, refv.clone())?;
                }
                Ok((refv, p2))
            }
            0x83 => {
                // DerefOf(operand)
                let (v, p1) = self.eval(f, p + 1)?;
                let out = match v {
                    Value::Ref(pl) => self.read_place(&pl)?,
                    other => other,
                };
                Ok((out, p1))
            }
            0x87 => {
                // SizeOf(SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let sz = match place {
                    Some(pl) => match self.read_place(&pl)? {
                        Value::Buffer(x) => x.len() as u64,
                        Value::Str(s) => s.len() as u64,
                        Value::Package(e) => e.len() as u64,
                        _ => 0,
                    },
                    None => 0,
                };
                Ok((Value::Int(sz), p1))
            }
            0x73 => {
                // Concatenate(a, b, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (tgt, p3) = self.super_name(f, p2)?;
                let r = concat(&a, &bb);
                if let Some(pl) = tgt {
                    self.store(&pl, r.clone())?;
                }
                Ok((r, p3))
            }
            0x99 => {
                // ToInteger(operand, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = Value::Int(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, r.clone())?;
                }
                Ok((r, p2))
            }
            0x86 => {
                // Notify(object, value) — no-op; consume both operands.
                let (_o, p1) = self.super_name(f, p + 1)?;
                let (_v, p2) = self.eval(f, p1)?;
                Ok((Value::Uninit, p2))
            }
            0x5B => self.eval_ext(f, p),
            // name-ish first byte -> NameString (method call or name/field read)
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => self.eval_name(f, p),
            other => Err(format!(
                "unhandled eval opcode {:#04x} at body+{:#x}",
                other, p
            )),
        }
    }

    fn eval_ext(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let ext = b[p + 1];
        match ext {
            0x23 => {
                // Acquire(mutex, timeout-u16) -> bool (0 = acquired)
                let (_pl, p1) = self.super_name(f, p + 2)?;
                Ok((Value::Int(0), p1 + 2))
            }
            0x27 => {
                // Release(mutex)
                let (_pl, p1) = self.super_name(f, p + 2)?;
                Ok((Value::Uninit, p1))
            }
            0x12 => {
                // CondRefOf(SuperName, target) -> bool
                let (place, p1) = self.super_name_opt(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let found = place.is_some();
                if let (Some(pl), Some(src)) = (tgt, place) {
                    self.store(&pl, Value::Ref(src))?;
                }
                Ok((Value::Int(found as u64), p2))
            }
            0x28 => {
                // FromBCD(value, target)
                let (a, p1) = self.eval(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = from_bcd(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x29 => {
                // ToBCD(value, target)
                let (a, p1) = self.eval(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = to_bcd(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x21 | 0x22 => {
                // Stall(usec) / Sleep(msec) — no-op, consume the time arg.
                let (_a, p1) = self.eval(f, p + 2)?;
                Ok((Value::Uninit, p1))
            }
            0x31 => {
                // DebugObj as a value (rare) — treat as 0.
                Ok((Value::Int(0), p + 2))
            }
            other => Err(format!("unhandled ext eval opcode 5B {:#04x} at body+{:#x}", other, p)),
        }
    }

    /// Evaluate a NameString as a value: method invocation, name read, or field read.
    fn eval_name(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let (nref, p1) = name_at(f.body, p);
        let path = self
            .ns
            .resolve(&f.scope, nref.rooted, nref.carets, &nref.segs)
            .ok_or_else(|| format!("unresolved name {} (scope {})", segs_str(&nref.segs), path_str(&f.scope)))?;
        match self.ns.get(&path) {
            Some(Node::Method { flags, .. }) => {
                let argc = (flags & 0x07) as usize;
                let mut args = Vec::with_capacity(argc);
                let mut q = p1;
                for _ in 0..argc {
                    let (v, nq) = self.eval(f, q)?;
                    args.push(obj(v));
                    q = nq;
                }
                let v = self.call_path(&path, args)?;
                Ok((v, q))
            }
            Some(Node::Name(v)) => Ok((v.borrow().clone(), p1)),
            Some(Node::Field { .. }) => Ok((self.read_field(&path)?, p1)),
            _ => Ok((Value::Uninit, p1)),
        }
    }

    /// Parse a SuperName / Target. Returns None for the null target (0x00) and
    /// for the Debug object (writes are discarded).
    fn super_name(&mut self, f: &Frame, p: usize) -> R<(Option<Place>, usize)> {
        self.super_name_opt(f, p)
    }

    fn super_name_opt(&mut self, f: &Frame, p: usize) -> R<(Option<Place>, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x00 => Ok((None, p + 1)), // NullName target
            0x60..=0x67 => Ok((Some(Place::Obj(f.locals[(op - 0x60) as usize].clone())), p + 1)),
            0x68..=0x6E => {
                let i = (op - 0x68) as usize;
                let o = if i < f.args.len() { f.args[i].clone() } else { obj(Value::Uninit) };
                Ok((Some(Place::Obj(o)), p + 1))
            }
            0x88 => {
                let (pl, p1) = self.index_place(f, p + 1)?;
                // optional nested target of Index is ignored when used as a target
                let (_t, p2) = self.super_name(f, p1)?;
                Ok((Some(pl), p2))
            }
            0x83 => {
                // DerefOf used as a target -> the referenced place
                let (v, p1) = self.eval(f, p + 1)?;
                match v {
                    Value::Ref(pl) => Ok((Some(pl), p1)),
                    _ => Ok((None, p1)),
                }
            }
            0x5B if b[p + 1] == 0x31 => Ok((None, p + 2)), // DebugObj sink
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {
                let (nref, p1) = name_at(b, p);
                let path = self.ns.resolve(&f.scope, nref.rooted, nref.carets, &nref.segs);
                match path {
                    Some(pp) => match self.ns.get(&pp) {
                        Some(Node::Field { .. }) => Ok((Some(Place::Field(pp)), p1)),
                        Some(Node::Name(o)) => Ok((Some(Place::Obj(o.clone())), p1)),
                        _ => Ok((Some(Place::Field(pp)), p1)),
                    },
                    None => Ok((None, p1)),
                }
            }
            _ => Err(format!("bad SuperName opcode {:#04x} at body+{:#x}", op, p)),
        }
    }

    /// Parse `Index(source, index)` into a Place (without the optional target).
    fn index_place(&mut self, f: &Frame, p: usize) -> R<(Place, usize)> {
        let (src, p1) = self.eval(f, p)?;
        let (idx, p2) = self.eval(f, p1)?;
        let i = idx.as_int() as usize;
        let src = match src {
            Value::Ref(pl) => self.read_place(&pl)?,
            other => other,
        };
        let place = match src {
            Value::Package(elems) => {
                if i < elems.len() {
                    Place::Obj(elems[i].clone())
                } else {
                    return Err(format!("package index {} out of range {}", i, elems.len()));
                }
            }
            Value::Buffer(_) | Value::Str(_) => {
                // Need the underlying cell for write-back. Best-effort: re-resolve
                // not possible from a value copy, so wrap a throwaway cell.
                Place::BufIndex(obj(src), i)
            }
            _ => return Err(String::from("Index on non-indexable value")),
        };
        Ok((place, p2))
    }

    // ── places ────────────────────────────────────────────────────────

    fn read_place(&mut self, pl: &Place) -> R<Value> {
        match pl {
            Place::Obj(o) => Ok(o.borrow().clone()),
            Place::Field(path) => self.read_field(path),
            Place::BufIndex(o, i) => {
                let v = o.borrow();
                match &*v {
                    Value::Buffer(b) => Ok(Value::Int(*b.get(*i).unwrap_or(&0) as u64)),
                    _ => Ok(Value::Int(0)),
                }
            }
        }
    }

    fn store(&mut self, pl: &Place, val: Value) -> R<()> {
        match pl {
            Place::Obj(o) => {
                *o.borrow_mut() = val;
                Ok(())
            }
            Place::Field(path) => self.write_field(path, val.as_int()),
            Place::BufIndex(o, i) => {
                let mut v = o.borrow_mut();
                if let Value::Buffer(b) = &mut *v {
                    if *i < b.len() {
                        b[*i] = val.as_int() as u8;
                    }
                }
                Ok(())
            }
        }
    }

    // ── field access via the EC region ────────────────────────────────

    fn region_byte(&mut self, space: u8, addr: u64) -> u8 {
        if space == 3 {
            self.ec.read(addr as u8)
        } else {
            *self.mem.get(&(space, addr)).unwrap_or(&0)
        }
    }

    fn set_region_byte(&mut self, space: u8, addr: u64, val: u8) {
        if space == 3 {
            self.ec.write(addr as u8, val);
        } else {
            self.mem.insert((space, addr), val);
        }
    }

    fn read_field(&mut self, path: &Path) -> R<Value> {
        let (space, base, bit_off, bit_w) = self.field_geom(path)?;
        let mut val: u64 = 0;
        let mut produced = 0u64;
        let mut bit = bit_off;
        while produced < bit_w {
            let addr = base + bit / 8;
            let bit_in = bit % 8;
            let take = core::cmp::min(8 - bit_in, bit_w - produced);
            let raw = self.region_byte(space, addr) as u64;
            let chunk = (raw >> bit_in) & ((1u64 << take) - 1);
            val |= chunk << produced;
            produced += take;
            bit += take;
        }
        Ok(Value::Int(val))
    }

    fn write_field(&mut self, path: &Path, val: u64) -> R<()> {
        let (space, base, bit_off, bit_w) = self.field_geom(path)?;
        let mut written = 0u64;
        let mut bit = bit_off;
        while written < bit_w {
            let addr = base + bit / 8;
            let bit_in = bit % 8;
            let take = core::cmp::min(8 - bit_in, bit_w - written);
            let mask = ((1u64 << take) - 1) << bit_in;
            let chunk = ((val >> written) & ((1u64 << take) - 1)) << bit_in;
            let mut cur = self.region_byte(space, addr) as u64;
            cur = (cur & !mask) | (chunk & mask);
            self.set_region_byte(space, addr, cur as u8);
            written += take;
            bit += take;
        }
        Ok(())
    }

    /// (region_space, region_byte_base, field_bit_offset, field_bit_width)
    fn field_geom(&self, path: &Path) -> R<(u8, u64, u64, u64)> {
        let (region, bit_offset, bit_width) = match self.ns.get(path) {
            Some(Node::Field { region, bit_offset, bit_width }) => {
                (region.clone(), *bit_offset, *bit_width)
            }
            _ => return Err(format!("{} is not a field", path_str(path))),
        };
        let (space, offset) = match self.ns.get(&region) {
            Some(Node::Region { space, offset, .. }) => (*space, *offset),
            _ => return Err(format!("region {} missing", path_str(&region))),
        };
        Ok((space, offset, bit_offset, bit_width))
    }
}

// ── free helpers ───────────────────────────────────────────────────────

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Uninit => "Uninit",
        Value::Int(_) => "Int",
        Value::Str(_) => "Str",
        Value::Buffer(_) => "Buffer",
        Value::Package(_) => "Package",
        Value::Ref(_) => "Ref",
    }
}

fn concat(a: &Value, b: &Value) -> Value {
    // Strings concatenate as strings; otherwise produce a buffer.
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => {
            let mut s = x.clone();
            s.push_str(y);
            Value::Str(s)
        }
        (Value::Str(x), other) => {
            let mut s = x.clone();
            s.push_str(&val_to_string(other));
            Value::Str(s)
        }
        (other, Value::Str(y)) => {
            let mut s = val_to_string(other);
            s.push_str(y);
            Value::Str(s)
        }
        _ => {
            let mut buf = to_bytes(a);
            buf.extend_from_slice(&to_bytes(b));
            Value::Buffer(buf)
        }
    }
}

fn val_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(n) => {
            // single-char if it's a small ASCII code (ISTR builds from NIST chars)
            if *n >= 0x20 && *n < 0x7f {
                let mut s = String::new();
                s.push(*n as u8 as char);
                s
            } else {
                let mut s = String::new();
                let mut x = *n;
                if x == 0 {
                    s.push('0');
                } else {
                    let mut tmp = [0u8; 20];
                    let mut i = 0;
                    while x > 0 {
                        tmp[i] = b'0' + (x % 10) as u8;
                        x /= 10;
                        i += 1;
                    }
                    while i > 0 {
                        i -= 1;
                        s.push(tmp[i] as char);
                    }
                }
                s
            }
        }
        Value::Buffer(b) => {
            let mut s = String::new();
            for &c in b {
                if c == 0 {
                    break;
                }
                s.push(c as char);
            }
            s
        }
        _ => String::new(),
    }
}

fn to_bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::Buffer(b) => b.clone(),
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(n) => n.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

fn to_bcd(mut n: u64) -> u64 {
    let mut r = 0u64;
    let mut shift = 0;
    while n > 0 {
        r |= (n % 10) << (shift * 4);
        n /= 10;
        shift += 1;
    }
    r
}

fn from_bcd(n: u64) -> u64 {
    let mut r = 0u64;
    let mut mul = 1u64;
    let mut x = n;
    while x > 0 {
        r += (x & 0x0F) * mul;
        x >>= 4;
        mul *= 10;
    }
    r
}

// ── NameString parsing (mirrors the loader) ─────────────────────────────

pub struct NRef {
    pub rooted: bool,
    pub carets: usize,
    pub segs: Vec<Seg>,
}

fn name_at(b: &[u8], mut p: usize) -> (NRef, usize) {
    let mut rooted = false;
    let mut carets = 0;
    if b[p] == 0x5C {
        rooted = true;
        p += 1;
    } else {
        while b[p] == 0x5E {
            carets += 1;
            p += 1;
        }
    }
    let mut segs: Vec<Seg> = Vec::new();
    match b[p] {
        0x00 => p += 1,
        0x2E => {
            p += 1;
            segs.push(seg_at(b, p));
            segs.push(seg_at(b, p + 4));
            p += 8;
        }
        0x2F => {
            p += 1;
            let count = b[p] as usize;
            p += 1;
            for i in 0..count {
                segs.push(seg_at(b, p + i * 4));
            }
            p += count * 4;
        }
        _ => {
            segs.push(seg_at(b, p));
            p += 4;
        }
    }
    (NRef { rooted, carets, segs }, p)
}

fn seg_at(b: &[u8], p: usize) -> Seg {
    let mut s: Seg = [0; 4];
    s.copy_from_slice(&b[p..p + 4]);
    s
}

fn segs_str(segs: &[Seg]) -> String {
    let mut s = String::new();
    for (i, sg) in segs.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        for &c in sg {
            s.push(c as char);
        }
    }
    s
}

fn pkg_length(b: &[u8], p: usize) -> (usize, usize) {
    let lead = b[p];
    let extra = (lead >> 6) as usize;
    if extra == 0 {
        ((p + (lead & 0x3F) as usize), p + 1)
    } else {
        let mut len = (lead & 0x0F) as usize;
        for i in 0..extra {
            len |= (b[p + 1 + i] as usize) << (4 + i * 8);
        }
        (p + len, p + 1 + extra)
    }
}

/// After a taken If-then block, skip a trailing Else block if present.
fn skip_else(b: &[u8], p: usize) -> usize {
    if p < b.len() && b[p] == 0xA1 {
        let (end, _p1) = pkg_length(b, p + 1);
        end
    } else {
        p
    }
}
