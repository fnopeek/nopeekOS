//! The encoder against LLVM's assembler: every case emits one or more words
//! through `Asm` and names the same instructions in assembly; `llvm-mc`
//! assembles the text, and the bytes must be the same.

extern crate std;

use super::*;
use std::format;
use std::string::String;
use std::vec::Vec;

fn x(n: u8) -> String {
    if n == 31 { "xzr".into() } else { format!("x{n}") }
}
fn w(n: u8) -> String {
    if n == 31 { "wzr".into() } else { format!("w{n}") }
}
fn xs(n: u8) -> String {
    if n == 31 { "sp".into() } else { format!("x{n}") }
}
fn reg(wide: bool, n: u8) -> String {
    if wide { x(n) } else { w(n) }
}
fn regsp(wide: bool, n: u8) -> String {
    if n == 31 { if wide { "sp".into() } else { "wsp".into() } } else { reg(wide, n) }
}

/// Assemble `lines` with llvm-mc; the bytes, in order.
fn llvm(lines: &[String]) -> Vec<u8> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("llvm-mc")
        .args(["-triple=aarch64", "-mattr=+v8.5a", "-show-encoding"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("llvm-mc");
    child.stdin.take().unwrap().write_all(lines.join("\n").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.trim().is_empty(), "llvm-mc: {err}");
    let text = String::from_utf8(out.stdout).unwrap();
    let mut bytes = Vec::new();
    for l in text.lines() {
        if let Some(enc) = l.split("encoding: [").nth(1) {
            for b in enc.trim_end_matches(']').split(',') {
                bytes.push(u8::from_str_radix(b.trim().trim_start_matches("0x"), 16).unwrap());
            }
        }
    }
    bytes
}

/// A batch of cases: our bytes and the text, compared at the end.
struct Cases {
    ours: Vec<(usize, usize, String)>,
    asm: Asm,
    text: Vec<String>,
}

impl Cases {
    fn new() -> Cases {
        Cases { ours: Vec::new(), asm: Asm::new(), text: Vec::new() }
    }
    fn case(&mut self, text: &str, f: impl FnOnce(&mut Asm)) {
        let a = self.asm.pos();
        f(&mut self.asm);
        let b = self.asm.pos();
        self.ours.push((a, b, text.into()));
        for l in text.split(';') {
            self.text.push(l.trim().into());
        }
    }
    fn check(self) {
        let want = llvm(&self.text);
        assert_eq!(want.len(), self.asm.buf.len(), "llvm assembled {} bytes, we {}", want.len(), self.asm.buf.len());
        for (a, b, t) in &self.ours {
            assert_eq!(&self.asm.buf[*a..*b], &want[*a..*b], "`{t}`");
        }
    }
}

const SAMPLE: [u8; 6] = [0, 1, 7, 17, 28, 30];

#[test]
fn data_processing_register() {
    let mut c = Cases::new();
    for &wide in &[false, true] {
        for &d in &SAMPLE {
            for &n in &[0u8, 5, 31] {
                for &m in &[3u8, 29, 31] {
                    let (dd, nn, mm) = (reg(wide, d), reg(wide, n), reg(wide, m));
                    c.case(&format!("add {dd}, {nn}, {mm}"), |a| a.add(wide, R(d), R(n), R(m)));
                    c.case(&format!("sub {dd}, {nn}, {mm}"), |a| a.sub(wide, R(d), R(n), R(m)));
                    c.case(&format!("subs {dd}, {nn}, {mm}"), |a| a.subs(wide, R(d), R(n), R(m)));
                    c.case(&format!("adds {dd}, {nn}, {mm}"), |a| a.adds(wide, R(d), R(n), R(m)));
                    c.case(&format!("and {dd}, {nn}, {mm}"), |a| a.and(wide, R(d), R(n), R(m)));
                    c.case(&format!("orr {dd}, {nn}, {mm}"), |a| a.orr(wide, R(d), R(n), R(m)));
                    c.case(&format!("eor {dd}, {nn}, {mm}"), |a| a.eor(wide, R(d), R(n), R(m)));
                    c.case(&format!("ands {dd}, {nn}, {mm}"), |a| a.ands(wide, R(d), R(n), R(m)));
                    c.case(&format!("bic {dd}, {nn}, {mm}"), |a| a.bic(wide, R(d), R(n), R(m)));
                    c.case(&format!("udiv {dd}, {nn}, {mm}"), |a| a.udiv(wide, R(d), R(n), R(m)));
                    c.case(&format!("sdiv {dd}, {nn}, {mm}"), |a| a.sdiv(wide, R(d), R(n), R(m)));
                    c.case(&format!("lsl {dd}, {nn}, {mm}"), |a| a.lslv(wide, R(d), R(n), R(m)));
                    c.case(&format!("lsr {dd}, {nn}, {mm}"), |a| a.lsrv(wide, R(d), R(n), R(m)));
                    c.case(&format!("asr {dd}, {nn}, {mm}"), |a| a.asrv(wide, R(d), R(n), R(m)));
                    c.case(&format!("ror {dd}, {nn}, {mm}"), |a| a.rorv(wide, R(d), R(n), R(m)));
                    c.case(&format!("mul {dd}, {nn}, {mm}"), |a| a.mul(wide, R(d), R(n), R(m)));
                    c.case(&format!("madd {dd}, {nn}, {mm}, {}", reg(wide, 9)), |a| a.madd(wide, R(d), R(n), R(m), R(9)));
                    c.case(&format!("msub {dd}, {nn}, {mm}, {}", reg(wide, 9)), |a| a.msub(wide, R(d), R(n), R(m), R(9)));
                    for &cc in &[Cond::Eq, Cond::Ne, Cond::Lo, Cond::Hi, Cond::Lt, Cond::Ge, Cond::Vs] {
                        let name = format!("{cc:?}").to_lowercase();
                        c.case(&format!("csel {dd}, {nn}, {mm}, {name}"), |a| a.csel(wide, R(d), R(n), R(m), cc));
                    }
                }
                let (dd, nn) = (reg(wide, d), reg(wide, n));
                c.case(&format!("clz {dd}, {nn}"), |a| a.clz(wide, R(d), R(n)));
                c.case(&format!("rbit {dd}, {nn}"), |a| a.rbit(wide, R(d), R(n)));
                c.case(&format!("mov {dd}, {nn}"), |a| a.mov(wide, R(d), R(n)));
                c.case(&format!("mvn {dd}, {nn}"), |a| a.mvn(wide, R(d), R(n)));
                c.case(&format!("neg {dd}, {nn}"), |a| a.neg(wide, R(d), R(n)));
                c.case(&format!("cmp {dd}, {nn}"), |a| a.cmp(wide, R(d), R(n)));
                c.case(&format!("tst {dd}, {nn}"), |a| a.tst(wide, R(d), R(n)));
                for &s in &[1u32, 5, 31] {
                    c.case(&format!("lsl {dd}, {nn}, #{s}"), |a| a.lsl_imm(wide, R(d), R(n), s));
                    c.case(&format!("lsr {dd}, {nn}, #{s}"), |a| a.lsr_imm(wide, R(d), R(n), s));
                    c.case(&format!("asr {dd}, {nn}, #{s}"), |a| a.asr_imm(wide, R(d), R(n), s));
                }
                c.case(&format!("sxtb {dd}, {}", w(n)), |a| a.sxt(wide, R(d), R(n), 8));
                c.case(&format!("sxth {dd}, {}", w(n)), |a| a.sxt(wide, R(d), R(n), 16));
                for &cc in &[Cond::Eq, Cond::Hi, Cond::Le] {
                    let name = format!("{cc:?}").to_lowercase();
                    c.case(&format!("cset {dd}, {name}"), |a| a.cset(wide, R(d), cc));
                }
            }
        }
        c.case(&format!("add {}, {}, {}, lsl #3", reg(wide, 1), reg(wide, 2), reg(wide, 3)),
            |a| a.add_shifted(wide, X1, X2, X3, Shift::Lsl, 3));
        c.case(&format!("orr {}, {}, {}, lsr #32", x(1), x(2), x(3)),
            |a| a.orr_shifted(true, X1, X2, X3, Shift::Lsr, 32));
    }
    for &d in &SAMPLE {
        c.case(&format!("sxtw {}, {}", x(d), w(5)), |a| a.sxt(true, R(d), X5, 32));
        c.case(&format!("uxtb {}, {}", w(d), w(5)), |a| a.uxt(R(d), X5, 8));
        c.case(&format!("uxth {}, {}", w(d), w(5)), |a| a.uxt(R(d), X5, 16));
        c.case(&format!("add {}, {}, {}, uxtw", xs(d), x(27), w(3)), |a| a.add_ext(true, R(d), X27, X3, Ext::Uxtw, 0));
        c.case(&format!("add {}, {}, {}, uxtw #3", xs(d), xs(31), w(3)), |a| a.add_ext(true, R(d), SP, X3, Ext::Uxtw, 3));
        c.case(&format!("sub {}, {}, {}, uxtw", xs(d), x(27), w(3)), |a| a.sub_ext(true, R(d), X27, X3, Ext::Uxtw, 0));
        c.case(&format!("sub {}, {}, {}", xs(d), xs(31), x(16)), |a| a.sub_ext(true, R(d), SP, X16, Ext::Uxtx, 0));
        c.case(&format!("add {}, {}, {}", xs(d), xs(31), x(16)), |a| a.add_ext(true, R(d), SP, X16, Ext::Uxtx, 0));
    }
    c.check();
}

#[test]
fn immediates() {
    let mut c = Cases::new();
    for &wide in &[false, true] {
        for &d in &[0u8, 17, 31] {
            for &n in &[1u8, 31] {
                for &imm in &[0u32, 1, 4095] {
                    for &sh in &[false, true] {
                        let lsl = if sh { ", lsl #12" } else { "" };
                        c.case(&format!("add {}, {}, #{imm}{lsl}", regsp(wide, d), regsp(wide, n)),
                            |a| a.add_imm(wide, R(d), R(n), imm, sh));
                        c.case(&format!("sub {}, {}, #{imm}{lsl}", regsp(wide, d), regsp(wide, n)),
                            |a| a.sub_imm(wide, R(d), R(n), imm, sh));
                        c.case(&format!("subs {}, {}, #{imm}{lsl}", reg(wide, d), regsp(wide, n)),
                            |a| a.subs_imm(wide, R(d), R(n), imm, sh));
                    }
                }
            }
            for hw in 0..(if wide { 4 } else { 2 }) {
                for &imm in &[0u32, 1, 0xffff, 0x1234] {
                    let r = reg(wide, d);
                    c.case(&format!("movz {r}, #{imm}, lsl #{}", 16 * hw), |a| a.movz(wide, R(d), imm, hw));
                    c.case(&format!("movk {r}, #{imm}, lsl #{}", 16 * hw), |a| a.movk(wide, R(d), imm, hw));
                    c.case(&format!("movn {r}, #{imm}, lsl #{}", 16 * hw), |a| a.movn(wide, R(d), imm, hw));
                }
            }
        }
        c.case(&format!("cmn {}, #17", reg(wide, 4)), |a| a.cmn_imm(wide, X4, 17));
        c.case(&format!("cmp {}, #4095", reg(wide, 4)), |a| a.cmp_imm(wide, X4, 4095));
    }
    c.case("mov sp, x29", |a| a.mov_sp(SP, FP));
    c.case("mov x29, sp", |a| a.mov_sp(FP, SP));
    c.check();
}

/// Every logical immediate `logical_imm` claims to encode, as LLVM encodes
/// it; and a few that must not encode.
#[test]
fn logical_immediates() {
    let mut c = Cases::new();
    let mut values: Vec<u64> = Vec::new();
    // Every element size, every run length, a few rotations.
    for size in [2u32, 4, 8, 16, 32, 64] {
        for ones in 1..size {
            for rot in [0, 1, size / 2, size - 1] {
                let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
                let run = (1u64 << ones) - 1;
                let elt = if rot == 0 { run } else { ((run >> rot) | (run << (size - rot))) & mask };
                let mut v = 0u64;
                let mut k = 0;
                while k < 64 {
                    v |= elt << k;
                    k += size;
                }
                values.push(v);
            }
        }
    }
    let mut n = 0;
    for &v in &values {
        for &wide in &[true, false] {
            let vv = if wide { v } else { v & 0xffff_ffff };
            if !wide && (v >> 32) != (v & 0xffff_ffff) { continue; }
            let e = logical_imm(vv, wide);
            assert!(e.is_some(), "{vv:#x} wide={wide} should encode");
            c.case(&format!("and {}, {}, #{vv:#x}", reg(wide, 1), reg(wide, 2)), |a| { assert!(a.and_imm(wide, X1, X2, vv)); });
            n += 1;
        }
    }
    assert!(n > 500);
    for bad in [0u64, u64::MAX, 0x1234_5678, 0x5, 0xff00_ff00_ff00_ff01] {
        assert!(logical_imm(bad, true).is_none(), "{bad:#x}");
    }
    c.check();
}

#[test]
fn loads_stores() {
    let mut c = Cases::new();
    let ms = [(Mem::B, "ldrb", "w", 0u32), (Mem::H, "ldrh", "w", 1), (Mem::W, "ldr", "w", 2), (Mem::X, "ldr", "x", 3),
        (Mem::SbW, "ldrsb", "w", 0), (Mem::SbX, "ldrsb", "x", 0), (Mem::ShW, "ldrsh", "w", 1),
        (Mem::ShX, "ldrsh", "x", 1), (Mem::SwX, "ldrsw", "x", 2)];
    for &(m, name, rk, scale) in &ms {
        for &t in &[0u8, 30] {
            for &n in &[3u8, 31] {
                let tt = if rk == "x" { x(t) } else { w(t) };
                for &off in &[0u32, 1 << scale, 4095 << scale] {
                    c.case(&format!("{name} {tt}, [{}, #{off}]", xs(n)), |a| a.ldr(m, R(t), R(n), off));
                }
                for &off in &[-256i32, -1, 255] {
                    let uname = name.replacen("ldr", "ldur", 1);
                    c.case(&format!("{uname} {tt}, [{}, #{off}]", xs(n)), |a| a.ldur(m, R(t), R(n), off));
                }
                c.case(&format!("{name} {tt}, [{}, {}, uxtw]", xs(n), w(5)), |a| a.ldr_reg(m, R(t), R(n), X5, Ext::Uxtw, false));
                if scale > 0 {
                    c.case(&format!("{name} {tt}, [{}, {}, uxtw #{scale}]", xs(n), w(5)), |a| a.ldr_reg(m, R(t), R(n), X5, Ext::Uxtw, true));
                }
                c.case(&format!("{name} {tt}, [{}, {}]", xs(n), x(5)), |a| a.ldr_reg(m, R(t), R(n), X5, Ext::Uxtx, false));
            }
        }
    }
    for &(m, name, rk, scale) in &[(Mem::B, "strb", "w", 0u32), (Mem::H, "strh", "w", 1), (Mem::W, "str", "w", 2), (Mem::X, "str", "x", 3)] {
        for &t in &[0u8, 31] {
            let tt = if rk == "x" { x(t) } else { w(t) };
            for &off in &[0u32, 4095 << scale] {
                c.case(&format!("{name} {tt}, [sp, #{off}]"), |a| a.str(m, R(t), SP, off));
            }
            let uname = name.replacen("str", "stur", 1);
            c.case(&format!("{uname} {tt}, [x2, #-8]"), |a| a.stur(m, R(t), X2, -8));
            c.case(&format!("{name} {tt}, [x27, w6, uxtw]"), |a| a.str_reg(m, R(t), X27, X6, Ext::Uxtw, false));
            c.case(&format!("{name} {tt}, [x16, x17]"), |a| a.str_reg(m, R(t), X16, X17, Ext::Uxtx, false));
            c.case(&format!("{name} {tt}, [sp, x17]"), |a| a.str_reg(m, R(t), SP, X17, Ext::Uxtx, false));
        }
    }
    for &off in &[-512i32, -16, 0, 504] {
        c.case(&format!("stp x29, x30, [sp, #{off}]!"), |a| a.stp_pre(FP, LR, SP, off));
        c.case(&format!("ldp x29, x30, [sp], #{off}"), |a| a.ldp_post(FP, LR, SP, off));
        c.case(&format!("stp x19, x20, [x29, #{off}]"), |a| a.stp(X19, X20, FP, off));
        c.case(&format!("ldp x19, x20, [x29, #{off}]"), |a| a.ldp(X19, X20, FP, off));
    }
    c.check();
}

#[test]
fn branches_and_system() {
    let mut c = Cases::new();
    for &off in &[-(1i64 << 27), -4, 0, 4, (1 << 27) - 4] {
        c.case(&format!("b #{off}"), |a| a.b_off(false, off));
        c.case(&format!("bl #{off}"), |a| a.b_off(true, off));
    }
    for &off in &[-(1i64 << 20), -8, 0, (1 << 20) - 4] {
        c.case(&format!("b.ne #{off}"), |a| a.bcond_off(Cond::Ne, off));
        c.case(&format!("b.hs #{off}"), |a| a.bcond_off(Cond::Hs, off));
        c.case(&format!("cbz w3, #{off}"), |a| a.cbz_off(false, false, X3, off));
        c.case(&format!("cbnz x30, #{off}"), |a| a.cbz_off(true, true, LR, off));
    }
    for &off in &[-(1i64 << 20), -3, 0, 7, (1 << 20) - 1] {
        c.case(&format!("adr x16, #{off}"), |a| a.adr_off(X16, off));
    }
    c.case("br x16", |a| a.br(X16));
    c.case("blr x17", |a| a.blr(X17));
    c.case("ret", |a| a.ret());
    c.case("brk #0x1234", |a| a.brk(0x1234));
    c.case("nop", |a| a.nop());
    c.case("csdb", |a| a.csdb());
    c.case("isb", |a| a.isb());
    c.check();
}

#[test]
fn forward_branches_bind() {
    // A bound branch must equal the directly encoded one.
    let mut a = Asm::new();
    let f1 = a.b();
    let f2 = a.bcond(Cond::Lt);
    let f3 = a.cbz(true, true, X9);
    let f4 = a.tbz(true, X3, 40);
    a.nop();
    a.nop();
    assert!(a.bind(f1) && a.bind(f2) && a.bind(f3) && a.bind(f4));
    let text: Vec<String> = ["b #24", "b.lt #20", "cbnz x9, #16", "tbnz x3, #40, #12", "nop", "nop"].iter().map(|s| (*s).into()).collect();
    assert_eq!(a.buf, llvm(&text));
}

#[test]
fn floating_point() {
    let mut c = Cases::new();
    for &(fw, p) in &[(Fw::S, "s"), (Fw::D, "d")] {
        let (d, n, m) = (format!("{p}0"), format!("{p}17"), format!("{p}31"));
        let (vd, vn, vm) = (V(0), V(17), V(31));
        c.case(&format!("fadd {d}, {n}, {m}"), |a| a.fadd(fw, vd, vn, vm));
        c.case(&format!("fsub {d}, {n}, {m}"), |a| a.fsub(fw, vd, vn, vm));
        c.case(&format!("fmul {d}, {n}, {m}"), |a| a.fmul(fw, vd, vn, vm));
        c.case(&format!("fdiv {d}, {n}, {m}"), |a| a.fdiv(fw, vd, vn, vm));
        c.case(&format!("fmin {d}, {n}, {m}"), |a| a.fmin(fw, vd, vn, vm));
        c.case(&format!("fmax {d}, {n}, {m}"), |a| a.fmax(fw, vd, vn, vm));
        c.case(&format!("fmov {d}, {n}"), |a| a.fmov(fw, vd, vn));
        c.case(&format!("fabs {d}, {n}"), |a| a.fabs(fw, vd, vn));
        c.case(&format!("fneg {d}, {n}"), |a| a.fneg(fw, vd, vn));
        c.case(&format!("fsqrt {d}, {n}"), |a| a.fsqrt(fw, vd, vn));
        c.case(&format!("frintn {d}, {n}"), |a| a.frintn(fw, vd, vn));
        c.case(&format!("frintp {d}, {n}"), |a| a.frintp(fw, vd, vn));
        c.case(&format!("frintm {d}, {n}"), |a| a.frintm(fw, vd, vn));
        c.case(&format!("frintz {d}, {n}"), |a| a.frintz(fw, vd, vn));
        c.case(&format!("fcmp {n}, {m}"), |a| a.fcmp(fw, vn, vm));
        c.case(&format!("fcsel {d}, {n}, {m}, gt"), |a| a.fcsel(fw, vd, vn, vm, Cond::Gt));
        for &wide in &[false, true] {
            let g = reg(wide, 5);
            c.case(&format!("scvtf {d}, {g}"), |a| a.scvtf(wide, fw, vd, X5));
            c.case(&format!("ucvtf {d}, {g}"), |a| a.ucvtf(wide, fw, vd, X5));
            c.case(&format!("fcvtzs {g}, {n}"), |a| a.fcvtzs(wide, fw, X5, vn));
            c.case(&format!("fcvtzu {g}, {n}"), |a| a.fcvtzu(wide, fw, X5, vn));
        }
        let g = if fw == Fw::D { "x9" } else { "w9" };
        c.case(&format!("fmov {d}, {g}"), |a| a.fmov_from_gpr(fw, vd, X9));
        c.case(&format!("fmov {g}, {n}"), |a| a.fmov_to_gpr(fw, X9, vn));
        let scale = if fw == Fw::D { 3 } else { 2 };
        for &off in &[0u32, 1 << scale, 4095 << scale] {
            c.case(&format!("ldr {d}, [x28, #{off}]"), |a| a.fldr(fw, vd, X28, off));
            c.case(&format!("str {n}, [sp, #{off}]"), |a| a.fstr(fw, vn, SP, off));
        }
        c.case(&format!("ldr {d}, [x27, w6, uxtw]"), |a| a.fldr_reg(fw, vd, X27, X6, Ext::Uxtw));
        c.case(&format!("str {d}, [x27, w6, uxtw]"), |a| a.fstr_reg(fw, vd, X27, X6, Ext::Uxtw));
        c.case(&format!("ldr {d}, [x29, x17]"), |a| a.fldr_reg(fw, vd, FP, X17, Ext::Uxtx));
        c.case(&format!("str {d}, [x16, x17]"), |a| a.fstr_reg(fw, vd, X16, X17, Ext::Uxtx));
        let z = if fw == Fw::D { "xzr" } else { "wzr" };
        c.case(&format!("fmov {d}, {z}"), |a| a.fmov_from_gpr(fw, vd, ZR));
    }
    c.case("fcvt d0, s17", |a| a.fcvt(Fw::S, V(0), V(17)));
    c.case("fcvt s0, d17", |a| a.fcvt(Fw::D, V(0), V(17)));
    c.case("stp x29, x30, [sp]", |a| a.stp(FP, LR, SP, 0));
    c.case("ldp x29, x30, [sp]", |a| a.ldp(FP, LR, SP, 0));
    c.case("ldrsw x17, [x16, w8, uxtw #2]", |a| a.ldr_reg(Mem::SwX, X17, X16, X8, Ext::Uxtw, true));
    c.case("ldr x16, [x16, w8, uxtw #3]", |a| a.ldr_reg(Mem::X, X16, X16, X8, Ext::Uxtw, true));
    c.case("ldr w16, [x16, w8, uxtw #2]", |a| a.ldr_reg(Mem::W, X16, X16, X8, Ext::Uxtw, true));
    c.case("ldr x0, [x29, x17]", |a| a.ldr_reg(Mem::X, X0, FP, X17, Ext::Uxtx, false));
    c.case("cnt v3.8b, v4.8b", |a| a.cnt_8b(V(3), V(4)));
    c.case("addv b3, v4.8b", |a| a.addv_8b(V(3), V(4)));
    c.check();
}

/// `mov_imm` leaves the right value: its words run on a small model of
/// movz/movn/movk/orr-immediate, for constants of every shape.
#[test]
fn mov_imm_values() {
    fn run(words: &[u32], wide: bool) -> u64 {
        let mut reg = 0u64;
        for &w in words {
            let hw = (w >> 21) & 3;
            let imm16 = ((w >> 5) & 0xffff) as u64;
            match (w >> 23) & 0x3f {
                0b100101 => match (w >> 29) & 3 {
                    0 => reg = !(imm16 << (16 * hw)),
                    2 => reg = imm16 << (16 * hw),
                    3 => reg = (reg & !(0xffff << (16 * hw))) | (imm16 << (16 * hw)),
                    _ => panic!("movw opc"),
                },
                0b100100 => {
                    // orr d, zr, #imm.
                    reg = decode_bitmask((w >> 10) & 0x1fff, wide);
                }
                _ => panic!("unexpected word {w:#x}"),
            }
            if !wide { reg &= 0xffff_ffff; }
        }
        reg
    }
    // Arm ARM DecodeBitMasks for the immediate case.
    fn decode_bitmask(field: u32, wide: bool) -> u64 {
        let n = (field >> 12) & 1;
        let immr = (field >> 6) & 0x3f;
        let imms = field & 0x3f;
        let len = 31 - ((n << 6) | (!imms & 0x3f)).leading_zeros();
        let size = 1u32 << len;
        let s = imms & (size - 1);
        let rr = immr & (size - 1);
        let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
        let elt = (1u64 << (s + 1)) - 1;
        let elt = if rr == 0 { elt } else { ((elt >> rr) | (elt << (size - rr))) & mask };
        let mut v = 0u64;
        let mut k = 0;
        while k < 64 { v |= elt << k; k += size; }
        if wide { v } else { v & 0xffff_ffff }
    }
    let mut vals: Vec<u64> = std::vec![0, 1, 0xffff, 0x10000, 0xffff_ffff, u64::MAX, 0x8000_0000_0000_0000,
        0x1234_5678_9abc_def0, 0xffff_ffff_0000_0000, 0x0000_ffff_0000_ffff, 0xfffe, 0xffff_ffff_ffff_fffe,
        0x5555_5555_5555_5555, 0x00ff_00ff_00ff_00ff, 0xdead_beef];
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..2000 {
        s ^= s << 13; s ^= s >> 7; s ^= s << 17;
        vals.push(s);
        vals.push(s & 0xffff_0000_ffff);
        vals.push(!(s & 0xffff));
    }
    for &v in &vals {
        for &wide in &[true, false] {
            let mut a = Asm::new();
            a.mov_imm(wide, X3, v);
            let words: Vec<u32> = a.buf.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let want = if wide { v } else { v & 0xffff_ffff };
            assert_eq!(run(&words, wide), want, "mov_imm({v:#x}, wide={wide}) = {words:x?}");
            assert!(words.len() <= if wide { 4 } else { 2 });
        }
    }
}
