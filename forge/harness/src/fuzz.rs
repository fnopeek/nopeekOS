//! Differential fuzzing: random valid modules under forge and under wasmi.
//!
//! The generator writes WAT from a seed, restricted to what forge translates,
//! and biased towards the places its code generation specialises: integer
//! locals that live in registers, comparisons feeding branches, immediates at
//! the 8-bit boundaries, branch distances around 127 bytes, values carried by
//! branches, and traps in the middle of side effects. Programs always
//! terminate: loops count down a counter nothing else touches, and calls only
//! go to functions defined later, so there is no recursion.
//!
//! Both engines must agree on the returned value, the trap, the whole linear
//! memory and every global. Float values are canonicalised wherever their bits
//! become observable, because wasm leaves NaN payloads open.
//!
//!   forge_harness --fuzz [start] [count] [jobs]   run a seed range
//!   forge_harness --fuzz-one <seed>               print one case and both outcomes

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    I32,
    I64,
    F32,
    F64,
}

impl T {
    fn name(self) -> &'static str {
        match self {
            T::I32 => "i32",
            T::I64 => "i64",
            T::F32 => "f32",
            T::F64 => "f64",
        }
    }
    fn is_float(self) -> bool {
        matches!(self, T::F32 | T::F64)
    }
}

const ALL: [T; 4] = [T::I32, T::I64, T::F32, T::F64];

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03 | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn pick<'a, X>(&mut self, xs: &'a [X]) -> &'a X {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

#[derive(Clone)]
struct Sig {
    params: Vec<T>,
    result: T,
}

#[derive(Clone, Copy, PartialEq)]
enum LabelKind {
    Block,
    Loop,
}

struct Label {
    name: String,
    kind: LabelKind,
    result: Option<T>,
    /// For a loop: the local counting it down.
    counter: Option<usize>,
}

const GLOBALS: [(&str, T); 5] = [
    ("$g0", T::I32),
    ("$g1", T::I32),
    ("$g2", T::I64),
    ("$g3", T::F32),
    ("$g4", T::F64),
];

struct Gen {
    rng: Rng,
    out: String,
    helpers: Vec<Sig>,
    /// Index of the function being written: `None` for `f`, else a helper.
    cur: Option<usize>,
    cur_result: T,
    locals: Vec<T>,
    /// Locals only the loop counters touch.
    reserved: Vec<bool>,
    labels: Vec<Label>,
    budget: i32,
    depth: u32,
    next_label: u32,
    table_len: usize,
    /// Loop counters in use, innermost last.
    busy: Vec<usize>,
}

const I32_EDGES: [i64; 18] = [
    0, 1, -1, 2, 7, 127, 128, -128, -129, 255, 256, 32767, 32768, 65535, 65536,
    0x7fff_ffff, -0x8000_0000, 0x1234_5678,
];
const I64_EDGES: [i64; 14] = [
    0, 1, -1, 127, 128, -128, -129, 0x7fff_ffff, 0x8000_0000, -0x8000_0000,
    -0x8000_0001, 0xffff_ffff, i64::MAX, i64::MIN,
];
const F_EDGES: [&str; 14] = [
    "0", "-0", "1", "-1", "0.5", "-0.5", "1.5", "2.5", "inf", "-inf", "nan",
    "3e9", "-3e9", "1e300",
];

impl Gen {
    fn emit(&mut self, s: &str) {
        self.out.push(' ');
        self.out.push_str(s);
        self.budget -= 1;
    }

    fn fresh_label(&mut self) -> String {
        self.next_label += 1;
        format!("$L{}", self.next_label)
    }

    fn leafy(&self) -> bool {
        self.budget <= 0 || self.depth >= 6
    }

    fn local_of(&mut self, t: T) -> Option<usize> {
        let c: Vec<usize> = (0..self.locals.len())
            .filter(|&i| self.locals[i] == t && !self.reserved[i])
            .collect();
        if c.is_empty() { None } else { Some(*self.rng.pick(&c)) }
    }

    fn konst(&mut self, t: T) {
        let s = match t {
            T::I32 => {
                let v = if self.rng.chance(60) {
                    *self.rng.pick(&I32_EDGES)
                } else {
                    self.rng.next() as i32 as i64
                };
                format!("i32.const {}", v as i32)
            }
            T::I64 => {
                let v = if self.rng.chance(60) {
                    *self.rng.pick(&I64_EDGES)
                } else if self.rng.chance(50) {
                    self.rng.next() as i32 as i64
                } else {
                    self.rng.next() as i64
                };
                format!("i64.const {v}")
            }
            T::F32 | T::F64 => {
                let v = if self.rng.chance(60) {
                    let e = *self.rng.pick(&F_EDGES);
                    if t == T::F32 && e == "1e300" { "3.4e38".to_string() } else { e.to_string() }
                } else {
                    format!("{}", (self.rng.next() as i32 as f64) / 64.0)
                };
                format!("{}.const {v}", t.name())
            }
        };
        self.emit(&s);
    }

    /// A memory address: mostly inside the first page, sometimes at its edge,
    /// rarely anywhere.
    fn addr(&mut self) -> String {
        let r = self.rng.below(100);
        if r < 40 {
            let a = self.rng.below(65536 - 16);
            self.emit(&format!("i32.const {a}"));
        } else if r < 98 {
            self.expr(T::I32);
            self.emit("i32.const 0x7ff8");
            self.emit("i32.and");
        } else {
            self.expr(T::I32);
        }
        let off = if self.rng.chance(3) {
            65530
        } else {
            *self.rng.pick(&[0u32, 0, 0, 1, 3, 4, 8, 127, 128, 4096])
        };
        if off == 0 { String::new() } else { format!(" offset={off}") }
    }

    fn canon(&mut self, t: T) {
        match t {
            T::F32 => self.emit("call $c32"),
            T::F64 => self.emit("call $c64"),
            _ => {}
        }
    }

    fn expr(&mut self, t: T) {
        self.depth += 1;
        self.expr_inner(t);
        self.depth -= 1;
    }

    fn expr_inner(&mut self, t: T) {
        if self.leafy() {
            match self.local_of(t) {
                Some(i) if self.rng.chance(60) => self.emit(&format!("local.get {i}")),
                _ => self.konst(t),
            }
            return;
        }
        let r = self.rng.below(100);
        match r {
            0..=11 => self.konst(t),
            12..=27 => match self.local_of(t) {
                Some(i) => self.emit(&format!("local.get {i}")),
                None => self.konst(t),
            },
            28..=31 => {
                let gs: Vec<&str> = GLOBALS.iter().filter(|g| g.1 == t).map(|g| g.0).collect();
                if self.rng.chance(20) && t == T::I32 {
                    self.emit("global.get $gk");
                } else {
                    let g = self.rng.pick(&gs).to_string();
                    self.emit(&format!("global.get {g}"));
                }
            }
            32..=53 => self.binop(t),
            54..=59 => self.unop(t),
            60..=67 => self.convert(t),
            68..=72 => self.load(t),
            73..=76 => {
                self.expr(t);
                self.expr(t);
                self.cond();
                self.emit("select");
            }
            77..=80 => match self.local_of(t) {
                Some(i) => {
                    self.expr(t);
                    self.emit(&format!("local.tee {i}"));
                }
                None => self.konst(t),
            },
            81..=84 => self.call_expr(t),
            85..=88 => {
                // if-else with a value.
                self.cond();
                let l = self.fresh_label();
                self.emit(&format!("if {l} (result {})", t.name()));
                self.labels.push(Label { name: l.clone(), kind: LabelKind::Block, result: Some(t), counter: None });
                self.stmts(2);
                self.expr(t);
                self.emit("else");
                self.stmts(2);
                self.expr(t);
                self.labels.pop();
                self.emit("end");
            }
            89..=93 => {
                // A block with a value, possibly leaving early with another.
                let l = self.fresh_label();
                self.emit(&format!("block {l} (result {})", t.name()));
                self.labels.push(Label { name: l.clone(), kind: LabelKind::Block, result: Some(t), counter: None });
                self.stmts(3);
                if self.rng.chance(60) {
                    self.expr(t);
                    self.cond();
                    self.emit(&format!("br_if {l}"));
                    self.emit("drop");
                    self.stmts(2);
                }
                self.expr(t);
                self.labels.pop();
                self.emit("end");
            }
            94..=95 if t == T::I32 => {
                if self.rng.chance(70) {
                    self.emit("memory.size");
                } else {
                    let d = self.rng.below(3);
                    self.emit(&format!("i32.const {d}"));
                    self.emit("memory.grow");
                }
            }
            _ if t == T::I32 => self.cmp(),
            _ => self.binop(t),
        }
    }

    /// An i32 that a branch or a select consumes.
    fn cond(&mut self) {
        if self.leafy() {
            self.expr(T::I32);
            return;
        }
        match self.rng.below(10) {
            0..=5 => self.cmp(),
            6 => {
                let u = *self.rng.pick(&[T::I32, T::I64]);
                self.expr(u);
                self.emit(&format!("{}.eqz", u.name()));
            }
            _ => self.expr(T::I32),
        }
    }

    fn cmp(&mut self) {
        let u = *self.rng.pick(&ALL);
        let ops: &[&str] = if u.is_float() {
            &["eq", "ne", "lt", "gt", "le", "ge"]
        } else {
            &["eq", "ne", "lt_s", "lt_u", "gt_s", "gt_u", "le_s", "le_u", "ge_s", "ge_u"]
        };
        let op = *self.rng.pick(ops);
        self.expr(u);
        if self.rng.chance(30) {
            self.konst(u);
        } else {
            self.expr(u);
        }
        self.emit(&format!("{}.{op}", u.name()));
    }

    fn binop(&mut self, t: T) {
        let ops: &[&str] = if t.is_float() {
            &["add", "sub", "mul", "div", "min", "max", "copysign"]
        } else {
            &[
                "add", "sub", "mul", "and", "or", "xor", "shl", "shr_s", "shr_u", "rotl",
                "rotr", "div_s", "div_u", "rem_s", "rem_u", "add", "sub", "and",
            ]
        };
        let op = *self.rng.pick(ops);
        self.expr(t);
        if self.rng.chance(35) {
            self.konst(t);
        } else {
            self.expr(t);
        }
        // Mostly a divisor that cannot be zero, so most programs run on;
        // `-1` stays possible for the overflow case.
        if !t.is_float() && (op.starts_with("div") || op.starts_with("rem")) {
            if self.rng.chance(85) {
                self.emit(&format!("{}.const 1", t.name()));
                self.emit(&format!("{}.or", t.name()));
            }
        }
        // The sign of a NaN is not specified, so a NaN must not lend its
        // sign to anything observable.
        if op == "copysign" {
            self.canon(t);
        }
        self.emit(&format!("{}.{op}", t.name()));
    }

    fn unop(&mut self, t: T) {
        let ops: &[&str] = match t {
            T::I32 => &["clz", "ctz", "popcnt", "eqz", "extend8_s", "extend16_s"],
            T::I64 => &["clz", "ctz", "popcnt", "extend8_s", "extend16_s", "extend32_s"],
            _ => &["abs", "neg", "ceil", "floor", "trunc", "nearest", "sqrt"],
        };
        let op = *self.rng.pick(ops);
        if op == "eqz" {
            let u = *self.rng.pick(&[T::I32, T::I64]);
            self.expr(u);
            self.emit(&format!("{}.eqz", u.name()));
            return;
        }
        self.expr(t);
        self.emit(&format!("{}.{op}", t.name()));
    }

    fn convert(&mut self, t: T) {
        let (from, op): (T, &str) = match t {
            T::I32 => *self.rng.pick(&[
                (T::I64, "i32.wrap_i64"),
                (T::F32, "i32.trunc_sat_f32_s"),
                (T::F32, "i32.trunc_sat_f32_u"),
                (T::F64, "i32.trunc_sat_f64_s"),
                (T::F64, "i32.trunc_sat_f64_u"),
                (T::F32, "i32.reinterpret_f32"),
            ]),
            T::I64 => *self.rng.pick(&[
                (T::I32, "i64.extend_i32_s"),
                (T::I32, "i64.extend_i32_u"),
                (T::F32, "i64.trunc_sat_f32_s"),
                (T::F64, "i64.trunc_sat_f64_s"),
                (T::F32, "i64.trunc_sat_f32_u"),
                (T::F64, "i64.trunc_sat_f64_u"),
                (T::F64, "i64.reinterpret_f64"),
            ]),
            T::F32 => *self.rng.pick(&[
                (T::I32, "f32.convert_i32_s"),
                (T::I32, "f32.convert_i32_u"),
                (T::I64, "f32.convert_i64_s"),
                (T::I64, "f32.convert_i64_u"),
                (T::F64, "f32.demote_f64"),
                (T::I32, "f32.reinterpret_i32"),
            ]),
            T::F64 => *self.rng.pick(&[
                (T::I32, "f64.convert_i32_s"),
                (T::I32, "f64.convert_i32_u"),
                (T::I64, "f64.convert_i64_s"),
                (T::I64, "f64.convert_i64_u"),
                (T::F32, "f64.promote_f32"),
                (T::I64, "f64.reinterpret_i64"),
            ]),
        };
        self.expr(from);
        if op.contains("reinterpret") {
            self.canon(from);
        }
        self.emit(op);
    }

    fn load(&mut self, t: T) {
        let op = match t {
            T::I32 => *self.rng.pick(&["i32.load", "i32.load8_s", "i32.load8_u", "i32.load16_s", "i32.load16_u"]),
            T::I64 => *self.rng.pick(&[
                "i64.load", "i64.load8_s", "i64.load8_u", "i64.load16_s", "i64.load16_u",
                "i64.load32_s", "i64.load32_u",
            ]),
            T::F32 => "f32.load",
            T::F64 => "f64.load",
        };
        let off = self.addr();
        self.emit(&format!("{op}{off}"));
    }

    fn call_expr(&mut self, t: T) {
        let from = self.cur.map(|c| c + 1).unwrap_or(0);
        let cands: Vec<usize> = (from..self.helpers.len()).filter(|&j| self.helpers[j].result == t).collect();
        if cands.is_empty() || self.budget < 4 {
            self.binop(t);
            return;
        }
        let j = *self.rng.pick(&cands);
        // Indirect calls only from `f`: every table entry is a helper, so no
        // cycle can form through the table. The signature named is mostly the
        // target's, sometimes another, which must trap.
        if self.cur.is_none() && self.rng.chance(35) {
            let sig = if self.rng.chance(85) { j } else { self.rng.below(self.helpers.len() as u64) as usize };
            for p in self.helpers[sig].params.clone() {
                self.expr(p);
            }
            let idx = if self.rng.chance(90) { j as u64 } else { self.rng.below(self.table_len as u64 + 3) };
            self.emit(&format!("i32.const {idx}"));
            self.emit(&format!("call_indirect (type $s{sig})"));
            if self.helpers[sig].result != t {
                self.emit("drop");
                self.konst(t);
            }
            return;
        }
        for p in self.helpers[j].params.clone() {
            self.expr(p);
        }
        self.emit(&format!("call $h{j}"));
    }

    fn stmts(&mut self, max: u64) {
        let n = self.rng.below(max + 1);
        for _ in 0..n {
            if self.budget <= 0 {
                return;
            }
            self.stmt();
        }
    }

    fn stmt(&mut self) {
        self.depth += 1;
        let deep = self.depth >= 5;
        let r = self.rng.below(100);
        match r {
            0..=21 => {
                let t = *self.rng.pick(&ALL);
                if let Some(i) = self.local_of(t) {
                    self.expr(t);
                    self.emit(&format!("local.set {i}"));
                }
            }
            22..=29 => {
                let (g, t) = *self.rng.pick(&GLOBALS);
                self.expr(t);
                self.canon(t);
                self.emit(&format!("global.set {g}"));
            }
            30..=41 => {
                let t = *self.rng.pick(&ALL);
                let op = match t {
                    T::I32 => *self.rng.pick(&["i32.store", "i32.store8", "i32.store16"]),
                    T::I64 => *self.rng.pick(&["i64.store", "i64.store8", "i64.store16", "i64.store32"]),
                    T::F32 => "f32.store",
                    T::F64 => "f64.store",
                };
                let off = self.addr();
                self.expr(t);
                self.canon(t);
                self.emit(&format!("{op}{off}"));
            }
            42..=50 if !deep => {
                self.cond();
                let l = self.fresh_label();
                self.emit(&format!("if {l}"));
                self.labels.push(Label { name: l, kind: LabelKind::Block, result: None, counter: None });
                self.stmts(4);
                if self.rng.chance(50) {
                    self.emit("else");
                    self.stmts(4);
                }
                self.labels.pop();
                self.emit("end");
            }
            51..=58 if !deep => {
                let l = self.fresh_label();
                self.emit(&format!("block {l}"));
                self.labels.push(Label { name: l, kind: LabelKind::Block, result: None, counter: None });
                self.stmts(5);
                self.labels.pop();
                self.emit("end");
            }
            59..=65 if !deep => self.counted_loop(),
            66..=72 => self.branch(),
            73..=74 => self.br_table(),
            75 => self.continue_loop(),
            76..=80 => {
                let t = *self.rng.pick(&ALL);
                self.call_expr(t);
                self.emit("drop");
            }
            81..=82 => {
                let (dst, val, len) = (self.addr_plain(), self.rng.below(256), self.len());
                self.emit(&format!("i32.const {dst}"));
                self.emit(&format!("i32.const {val}"));
                self.emit(&format!("i32.const {len}"));
                self.emit("memory.fill");
            }
            83..=84 => {
                let (dst, src, len) = (self.addr_plain(), self.addr_plain(), self.len());
                self.emit(&format!("i32.const {dst}"));
                self.emit(&format!("i32.const {src}"));
                self.emit(&format!("i32.const {len}"));
                self.emit("memory.copy");
            }
            85 => {
                let t = self.cur_result;
                if self.rng.chance(40) {
                    self.expr(t);
                    self.emit("return");
                }
            }
            86 => {
                if self.rng.chance(10) {
                    self.emit("unreachable");
                }
            }
            87..=91 => {
                let t = *self.rng.pick(&ALL);
                self.expr(t);
                self.emit("drop");
            }
            _ => {
                // Pad with straight-line work so later jumps cross the
                // rel8 range.
                let n = self.rng.below(12);
                for _ in 0..n {
                    let t = *self.rng.pick(&[T::I32, T::I64]);
                    if let Some(i) = self.local_of(t) {
                        self.emit(&format!("local.get {i}"));
                        self.konst(t);
                        let op = *self.rng.pick(&["add", "xor", "mul", "sub"]);
                        self.emit(&format!("{}.{op}", t.name()));
                        self.emit(&format!("local.set {i}"));
                    }
                }
            }
        }
        self.depth -= 1;
    }

    fn addr_plain(&mut self) -> u64 {
        if self.rng.chance(95) { self.rng.below(65536 - 64) } else { 65536 - self.rng.below(40) }
    }

    fn len(&mut self) -> u64 {
        if self.rng.chance(95) { self.rng.below(64) } else { self.rng.below(200) }
    }

    /// A label a branch may target: never a loop, whose branch would restart
    /// it and could run forever.
    fn targets(&self) -> Vec<usize> {
        (0..self.labels.len()).filter(|&i| self.labels[i].kind == LabelKind::Block).collect()
    }

    fn branch(&mut self) {
        let ts = self.targets();
        if ts.is_empty() {
            return;
        }
        let i = *self.rng.pick(&ts);
        let (name, result) = (self.labels[i].name.clone(), self.labels[i].result);
        if let Some(t) = result {
            self.expr(t);
        }
        if self.rng.chance(75) {
            self.cond();
            self.emit(&format!("br_if {name}"));
            if result.is_some() {
                self.emit("drop");
            }
        } else {
            self.emit(&format!("br {name}"));
        }
    }

    fn br_table(&mut self) {
        let ts = self.targets();
        if ts.is_empty() {
            return;
        }
        let i = *self.rng.pick(&ts);
        let result = self.labels[i].result;
        let same: Vec<usize> = ts.into_iter().filter(|&k| self.labels[k].result == result).collect();
        let n = 1 + self.rng.below(6);
        let mut names = String::new();
        for _ in 0..n {
            let k = *self.rng.pick(&same);
            let _ = write!(names, " {}", self.labels[k].name);
        }
        if let Some(t) = result {
            self.expr(t);
        }
        if self.rng.chance(50) {
            self.expr(T::I32);
        } else {
            let v = self.rng.below(n + 3);
            self.emit(&format!("i32.const {v}"));
        }
        self.emit(&format!("br_table{names}"));
    }

    fn counted_loop(&mut self) {
        let Some(c) = (0..self.locals.len()).find(|&i| self.reserved[i] && self.locals[i] == T::I32 && !self.counter_busy(i)) else {
            return;
        };
        self.busy.push(c);
        let n = 1 + self.rng.below(6);
        self.emit(&format!("i32.const {n}"));
        self.emit(&format!("local.set {c}"));
        let l = self.fresh_label();
        if self.rng.chance(50) {
            // do-while: the test at the bottom branches back.
            self.emit(&format!("loop {l}"));
            self.labels.push(Label { name: l.clone(), kind: LabelKind::Loop, result: None, counter: Some(c) });
            self.stmts(5);
            self.step(c);
            self.emit(&format!("br_if {l}"));
            self.labels.pop();
            self.emit("end");
        } else {
            // while: the test at the top leaves, the bottom jumps back
            // unconditionally.
            let b = self.fresh_label();
            self.emit(&format!("block {b}"));
            self.labels.push(Label { name: b.clone(), kind: LabelKind::Block, result: None, counter: None });
            self.emit(&format!("loop {l}"));
            self.labels.push(Label { name: l.clone(), kind: LabelKind::Loop, result: None, counter: Some(c) });
            self.emit(&format!("local.get {c}"));
            self.emit("i32.const 0");
            self.emit("i32.le_s");
            self.emit(&format!("br_if {b}"));
            self.stmts(5);
            self.emit(&format!("local.get {c}"));
            self.emit("i32.const 1");
            self.emit("i32.sub");
            self.emit(&format!("local.set {c}"));
            self.emit(&format!("br {l}"));
            self.labels.pop();
            self.emit("end");
            self.labels.pop();
            self.emit("end");
        }
        self.busy.pop();
    }

    /// Count `c` down and leave "is there another round" on the stack. The
    /// test is signed, so a counter already at zero cannot wrap into four
    /// billion more rounds.
    fn step(&mut self, c: usize) {
        self.emit(&format!("local.get {c}"));
        self.emit("i32.const 1");
        self.emit("i32.sub");
        self.emit(&format!("local.tee {c}"));
        self.emit("i32.const 0");
        self.emit("i32.gt_s");
    }

    /// `continue` to an enclosing loop: its counter goes down first, so the
    /// loop still ends.
    fn continue_loop(&mut self) {
        let ls: Vec<usize> = (0..self.labels.len()).filter(|&i| self.labels[i].kind == LabelKind::Loop).collect();
        if ls.is_empty() {
            return;
        }
        let i = *self.rng.pick(&ls);
        let (name, c) = (self.labels[i].name.clone(), self.labels[i].counter.unwrap());
        self.step(c);
        if self.rng.chance(50) {
            self.emit(&format!("br_if {name}"));
        } else {
            self.emit("if");
            self.emit(&format!("br {name}"));
            self.emit("end");
        }
    }

    fn counter_busy(&self, i: usize) -> bool {
        self.busy.contains(&i)
    }
}

impl Gen {
    fn new(seed: u64) -> Gen {
        let mut rng = Rng::new(seed);
        let nh = 1 + rng.below(5) as usize;
        let mut helpers = Vec::new();
        for _ in 0..nh {
            let np = rng.below(4) as usize;
            let params = (0..np).map(|_| *rng.pick(&ALL)).collect();
            let result = *rng.pick(&ALL);
            helpers.push(Sig { params, result });
        }
        Gen {
            rng,
            out: String::new(),
            table_len: nh,
            helpers,
            cur: None,
            cur_result: T::I32,
            locals: Vec::new(),
            reserved: Vec::new(),
            labels: Vec::new(),
            budget: 0,
            depth: 0,
            next_label: 0,
            busy: Vec::new(),
        }
    }

    fn func(&mut self, name: &str, params: &[T], result: T, export: bool) {
        self.locals = params.to_vec();
        self.reserved = vec![false; params.len()];
        let nl = self.rng.below(12) as usize;
        let mut decl = String::new();
        for _ in 0..nl {
            let t = *self.rng.pick(&ALL);
            self.locals.push(t);
            self.reserved.push(false);
            let _ = write!(decl, " (local {})", t.name());
        }
        for _ in 0..3 {
            self.locals.push(T::I32);
            self.reserved.push(true);
            decl.push_str(" (local i32)");
        }
        let mut head = format!("  (func {name}");
        if export {
            head.push_str(" (export \"f\")");
        }
        for p in params {
            let _ = write!(head, " (param {})", p.name());
        }
        let _ = write!(head, " (result {}){decl}\n   ", result.name());
        self.out.push_str(&head);
        self.cur_result = result;
        self.budget = 40 + self.rng.below(260) as i32;
        self.depth = 0;
        self.stmts(12);
        self.budget = self.budget.max(6);
        self.expr(result);
        if export {
            // Fold every local into the result, so a value that lived in a
            // register and was never written back shows up.
            for i in 0..self.locals.len() {
                let t = self.locals[i];
                self.emit(&format!("local.get {i}"));
                self.canon(t);
                match t {
                    T::I32 => {}
                    T::I64 => self.emit("i32.wrap_i64"),
                    T::F32 => self.emit("i32.reinterpret_f32"),
                    T::F64 => {
                        self.emit("i64.reinterpret_f64");
                        self.emit("i32.wrap_i64");
                    }
                }
                self.emit("i32.xor");
                self.emit("i32.const 5");
                self.emit("i32.rotl");
            }
        }
        self.out.push_str(")\n");
    }

    fn module(mut self) -> String {
        self.out.push_str("(module\n");
        self.out.push_str("  (import \"env\" \"add\" (func $add (param i32 i32) (result i32)))\n");
        for (i, s) in self.helpers.clone().iter().enumerate() {
            let mut ty = format!("  (type $s{i} (func");
            for p in &s.params {
                let _ = write!(ty, " (param {})", p.name());
            }
            let _ = writeln!(ty, " (result {})))", s.result.name());
            self.out.push_str(&ty);
        }
        self.out.push_str("  (memory (export \"m\") 1 2)\n");
        for (g, t) in GLOBALS {
            let init = match t {
                T::I32 => format!("{}", self.rng.next() as i32),
                T::I64 => format!("{}", self.rng.next() as i64),
                _ => "1.5".to_string(),
            };
            let _ = writeln!(self.out, "  (global {g} (export \"{}\") (mut {}) ({}.const {init}))", &g[1..], t.name(), t.name());
        }
        self.out.push_str("  (global $gk i32 (i32.const 77))\n");
        let n = self.table_len;
        let _ = writeln!(self.out, "  (table {n} funcref)");
        let mut elems = String::new();
        for j in 0..n {
            let _ = write!(elems, " $h{j}");
        }
        let _ = writeln!(self.out, "  (elem (i32.const 0) func{elems})");
        let mut bytes = String::new();
        for _ in 0..64 {
            let _ = write!(bytes, "\\{:02x}", self.rng.below(256));
        }
        let _ = writeln!(self.out, "  (data (i32.const 0) \"{bytes}\")");
        let _ = writeln!(self.out, "  (data (i32.const 65472) \"{bytes}\")");
        self.out.push_str(
            "  (func $c32 (param f32) (result f32) local.get 0 local.get 0 f32.eq if (result f32) local.get 0 else f32.const nan end)\n",
        );
        self.out.push_str(
            "  (func $c64 (param f64) (result f64) local.get 0 local.get 0 f64.eq if (result f64) local.get 0 else f64.const nan end)\n",
        );

        self.cur = None;
        self.func("$f", &[T::I32], T::I32, true);
        for j in 0..self.helpers.len() {
            self.cur = Some(j);
            let s = self.helpers[j].clone();
            self.func(&format!("$h{j}"), &s.params, s.result, false);
        }
        self.out.push_str(")\n");
        self.out
    }
}

/// What a run left behind.
#[derive(PartialEq, Debug)]
struct Outcome {
    trap: u32,
    result: u32,
    memory: Vec<u8>,
    globals: Vec<u64>,
}

fn under_forge(wasm: &[u8], arg: u32) -> Result<Outcome, String> {
    use crate::selftest::{Exec, Inst};
    let m = forge_core::compile(wasm).map_err(|e| format!("compile: {e}"))?;
    for o in &m.funcs {
        if let forge_core::codegen::Outcome::Unsupported(n) = o {
            return Err(format!("refused: {n}"));
        }
    }
    let fidx = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i).ok_or("no f")?;
    let off = m.offset_of(fidx).ok_or("f not compiled")?;
    let exec = Exec::new(&m.code).ok_or("exec")?;
    exec.arm_traps(&m);
    let mut inst = Inst::new(&m, exec.base()).ok_or("inst")?;
    inst.set_stack_budget(512 * 1024);
    let r = exec.call_entry(m.entry_offset, off, inst.ptr(), arg, 0, 0);
    let trap = inst.trap_code();
    let globals = (0..GLOBALS.len())
        .map(|i| {
            let v = inst.global(i);
            match GLOBALS[i].1 {
                T::I32 | T::F32 => v & 0xffff_ffff,
                _ => v,
            }
        })
        .collect();
    Ok(Outcome {
        trap,
        result: if trap == forge_core::trap::NONE { r } else { 0 },
        memory: inst.memory_bytes().to_vec(),
        globals,
    })
}

fn under_wasmi(wasm: &[u8], arg: u32) -> Result<Outcome, String> {
    use wasmi::TrapCode as W;
    use wasmi::{Config, Engine, Linker, Module, Store, Val};
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    let engine = Engine::new(&cfg);
    let module = Module::new(&engine, wasm).map_err(|e| format!("wasmi module: {e}"))?;
    let mut store = Store::new(&engine, ());
    store.set_fuel(u64::MAX / 4).map_err(|e| e.to_string())?;
    let mut linker = <Linker<()>>::new(&engine);
    linker
        .func_wrap("env", "add", |a: i32, b: i32| a.wrapping_add(b))
        .map_err(|e| e.to_string())?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| format!("wasmi instantiate: {e}"))?;
    let f = instance.get_func(&store, "f").ok_or("no f")?;
    let mut out = [Val::I32(0)];
    let (trap, result) = match f.call(&mut store, &[Val::I32(arg as i32)], &mut out) {
        Ok(()) => match out[0] {
            Val::I32(v) => (forge_core::trap::NONE, v as u32),
            _ => return Err("result type".into()),
        },
        Err(e) => {
            use forge_core::trap as t;
            let code = match e.as_trap_code() {
                Some(W::UnreachableCodeReached) => t::UNREACHABLE,
                Some(W::MemoryOutOfBounds) => t::MEMORY_OUT_OF_BOUNDS,
                Some(W::TableOutOfBounds) | Some(W::IndirectCallToNull) => t::TABLE_OUT_OF_BOUNDS,
                Some(W::IntegerDivisionByZero) | Some(W::IntegerOverflow) => t::DIVIDE_ERROR,
                Some(W::BadSignature) => t::BAD_SIGNATURE,
                Some(W::StackOverflow) => t::STACK_EXHAUSTED,
                Some(W::OutOfFuel) => t::OUT_OF_FUEL,
                _ => return Err(format!("wasmi error: {e}")),
            };
            (code, 0)
        }
    };
    let mem = instance.get_memory(&store, "m").ok_or("no m")?;
    let memory = mem.data(&store).to_vec();
    let mut globals = Vec::new();
    for (g, t) in GLOBALS {
        let v = instance.get_global(&store, &g[1..]).ok_or("no global")?.get(&store);
        globals.push(match (v, t) {
            (Val::I32(x), _) => x as u32 as u64,
            (Val::I64(x), _) => x as u64,
            (Val::F32(x), _) => x.to_bits() as u64,
            (Val::F64(x), _) => x.to_bits(),
            _ => return Err("global type".into()),
        });
    }
    Ok(Outcome { trap, result, memory, globals })
}

enum Verdict {
    Agree(u32),
    Refused(String),
    Invalid(String),
    Differ(String),
}

fn arg_for(seed: u64) -> u32 {
    [0u32, 1, 7, 0xffff_ffff, 128, 0x8000_0000][(seed % 6) as usize]
}

fn check(seed: u64) -> (String, Verdict) {
    let wat = Gen::new(seed).module();
    let wasm = match wat::parse_str(&wat) {
        Ok(w) => w,
        Err(e) => return (wat, Verdict::Invalid(format!("wat: {e}"))),
    };
    let arg = arg_for(seed);
    let w = match under_wasmi(&wasm, arg) {
        Ok(o) => o,
        Err(e) => return (wat, Verdict::Invalid(e)),
    };
    let f = match under_forge(&wasm, arg) {
        Ok(o) => o,
        Err(e) if e.starts_with("refused") => return (wat, Verdict::Refused(e)),
        Err(e) => return (wat, Verdict::Differ(format!("forge failed: {e}"))),
    };
    if f == w {
        return (wat, Verdict::Agree(f.trap));
    }
    let mut d = String::new();
    if f.trap != w.trap {
        let _ = write!(d, "trap forge {} wasmi {}; ", forge_core::trap::name(f.trap), forge_core::trap::name(w.trap));
    }
    if f.result != w.result {
        let _ = write!(d, "result forge {:#x} wasmi {:#x}; ", f.result, w.result);
    }
    if f.memory.len() != w.memory.len() {
        let _ = write!(d, "memory size forge {} wasmi {}; ", f.memory.len(), w.memory.len());
    } else if let Some(i) = (0..f.memory.len()).find(|&i| f.memory[i] != w.memory[i]) {
        let _ = write!(d, "memory first differs at {i}: forge {:#x} wasmi {:#x}; ", f.memory[i], w.memory[i]);
    }
    for i in 0..GLOBALS.len() {
        if f.globals[i] != w.globals[i] {
            let _ = write!(d, "{} forge {:#x} wasmi {:#x}; ", GLOBALS[i].0, f.globals[i], w.globals[i]);
        }
    }
    (wat, Verdict::Differ(d))
}

/// One worker: runs seeds in-process and names each before it starts, so a
/// crash in generated code still points at its seed.
fn worker(start: u64, count: u64) -> bool {
    crate::faults::install();
    let stdout = std::io::stdout();
    for seed in start..start + count {
        {
            let mut o = stdout.lock();
            let _ = writeln!(o, "S {seed}");
            let _ = o.flush();
        }
        let (_, v) = check(seed);
        let line = match v {
            Verdict::Agree(t) => format!("A {seed} {t}"),
            Verdict::Refused(m) => format!("R {seed} {m}"),
            Verdict::Invalid(m) => format!("I {seed} {}", m.replace('\n', " ")),
            Verdict::Differ(m) => format!("D {seed} {m}"),
        };
        let mut o = stdout.lock();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    true
}

pub fn run(args: &[String]) -> bool {
    if args.first().map(|a| a == "--worker").unwrap_or(false) {
        return worker(args[1].parse().unwrap(), args[2].parse().unwrap());
    }
    let start: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
    let count: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let jobs: u64 = args
        .get(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(4));
    let exe = std::env::current_exe().expect("own path");
    let per = count.div_ceil(jobs);
    let t0 = std::time::Instant::now();
    let handles: Vec<_> = (0..jobs)
        .map(|j| {
            let exe = exe.clone();
            std::thread::spawn(move || {
                let mut lines = Vec::new();
                let lo = start + j * per;
                let hi = (lo + per).min(start + count);
                let mut next = lo;
                while next < hi {
                    let mut child = Command::new(&exe)
                        .args(["--fuzz", "--worker", &next.to_string(), &(hi - next).to_string()])
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("spawn worker");
                    // A wild jump in generated code can loop where no fuel
                    // check ever runs, so a case that does not finish in time
                    // is a finding too.
                    let (tx, rx) = std::sync::mpsc::channel();
                    let out = child.stdout.take().unwrap();
                    std::thread::spawn(move || {
                        for l in BufReader::new(out).lines() {
                            if tx.send(l.unwrap_or_default()).is_err() {
                                break;
                            }
                        }
                    });
                    let mut last = None;
                    let mut hung = false;
                    loop {
                        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                            Ok(l) => {
                                if let Some(s) = l.strip_prefix("S ") {
                                    last = s.parse::<u64>().ok();
                                } else {
                                    lines.push(l);
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                let _ = child.kill();
                                hung = true;
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    let status = child.wait().expect("wait");
                    if status.success() && !hung {
                        break;
                    }
                    let s = last.unwrap_or(next);
                    if hung {
                        lines.push(format!("H {s} did not finish"));
                    } else {
                        lines.push(format!("C {s} crashed ({status})"));
                    }
                    next = s + 1;
                }
                lines
            })
        })
        .collect();
    let mut all: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    all.sort_by_key(|l| l.split(' ').nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    let mut tally = std::collections::BTreeMap::<char, u64>::new();
    let mut reasons = std::collections::BTreeMap::<String, u64>::new();
    for l in &all {
        let k = l.chars().next().unwrap_or('?');
        *tally.entry(k).or_default() += 1;
        if k == 'A' {
            let t: u32 = l.split(' ').nth(2).and_then(|x| x.parse().ok()).unwrap_or(0);
            *reasons.entry(format!("A ends: {}", forge_core::trap::name(t))).or_default() += 1;
        }
        if k == 'R' || k == 'I' {
            let r: String = l.splitn(3, ' ').nth(2).unwrap_or("").split(" at <anon>").next().unwrap_or("").chars().take(70).collect();
            *reasons.entry(format!("{k} {r}")).or_default() += 1;
        }
    }
    let bad: Vec<&String> = all.iter().filter(|l| l.starts_with(['D', 'C', 'H'])).collect();
    for l in bad.iter().take(30) {
        println!("{l}");
    }
    for (r, n) in &reasons {
        println!("{n:>6}  {r}");
    }
    let n = |c| tally.get(&c).copied().unwrap_or(0);
    println!(
        "fuzz {start}..{}: {} agree, {} differ, {} crash, {} hang, {} refused, {} invalid ({:.1} s)",
        start + count,
        n('A'),
        n('D'),
        n('C'),
        n('H'),
        n('R'),
        n('I'),
        t0.elapsed().as_secs_f64()
    );
    bad.is_empty()
}

pub fn one(seed: u64) -> bool {
    crate::faults::install();
    let (wat, v) = check(seed);
    println!("{wat}");
    println!("arg {:#x}", arg_for(seed));
    match v {
        Verdict::Agree(_) => {
            println!("agree");
            true
        }
        Verdict::Refused(m) => {
            println!("{m}");
            true
        }
        Verdict::Invalid(m) => {
            println!("invalid: {m}");
            false
        }
        Verdict::Differ(m) => {
            println!("DIFFER: {m}");
            false
        }
    }
}
