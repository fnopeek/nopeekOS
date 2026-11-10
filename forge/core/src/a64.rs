//! AArch64 (A64) encoder. Just the instructions the generator emits — not an
//! assembler. Every encoding here is checked bit for bit against LLVM's
//! assembler (`a64_tests`), the field layouts are the Arm ARM's (DDI 0487,
//! C4 "A64 Instruction Set Encoding").
//!
//! Every instruction is one 32-bit word, little endian. Register 31 is the
//! zero register or the stack pointer depending on the instruction; the
//! functions say which they take.

use alloc::vec::Vec;

/// A general-purpose register number, 0..=30, or 31 (`ZR`/`SP`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct R(pub u8);

pub const X0: R = R(0);
pub const X1: R = R(1);
pub const X2: R = R(2);
pub const X3: R = R(3);
pub const X4: R = R(4);
pub const X5: R = R(5);
pub const X6: R = R(6);
pub const X7: R = R(7);
pub const X8: R = R(8);
pub const X9: R = R(9);
pub const X10: R = R(10);
pub const X11: R = R(11);
pub const X12: R = R(12);
pub const X13: R = R(13);
pub const X14: R = R(14);
pub const X15: R = R(15);
pub const X16: R = R(16);
pub const X17: R = R(17);
pub const X19: R = R(19);
pub const X20: R = R(20);
pub const X21: R = R(21);
pub const X22: R = R(22);
pub const X23: R = R(23);
pub const X24: R = R(24);
pub const X25: R = R(25);
pub const X26: R = R(26);
pub const X27: R = R(27);
pub const X28: R = R(28);
pub const FP: R = R(29);
pub const LR: R = R(30);
/// Register 31 where an instruction reads it as zero.
pub const ZR: R = R(31);
/// Register 31 where an instruction reads it as the stack pointer.
pub const SP: R = R(31);

/// A SIMD/FP register number, 0..=31, used as `s`/`d`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct V(pub u8);

/// Condition codes, the 4-bit field of `b.cond`/`csel`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Cond {
    Eq = 0x0,
    Ne = 0x1,
    /// Carry set / unsigned higher or same.
    Hs = 0x2,
    /// Carry clear / unsigned lower.
    Lo = 0x3,
    Mi = 0x4,
    Pl = 0x5,
    Vs = 0x6,
    Vc = 0x7,
    Hi = 0x8,
    Ls = 0x9,
    Ge = 0xa,
    Lt = 0xb,
    Gt = 0xc,
    Le = 0xd,
    Al = 0xe,
}

impl Cond {
    /// The condition that holds exactly when this one does not: A64 pairs
    /// them in the low bit (except AL).
    pub fn not(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Hs => Cond::Lo,
            Cond::Lo => Cond::Hs,
            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,
            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,
            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,
            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,
            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,
            Cond::Al => Cond::Al,
        }
    }
}

/// Shift of a shifted-register operand.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum Shift {
    Lsl = 0,
    Lsr = 1,
    Asr = 2,
    Ror = 3,
}

/// Extend of an extended-register operand or a register-offset address.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum Ext {
    Uxtb = 0,
    Uxth = 1,
    Uxtw = 2,
    Uxtx = 3,
    Sxtb = 4,
    Sxth = 5,
    Sxtw = 6,
    Sxtx = 7,
}

/// Size of a load or store: the access, and for a load whether it sign
/// extends into a 32- or 64-bit register.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Mem {
    /// `strb`/`ldrb`.
    B,
    /// `strh`/`ldrh`.
    H,
    /// `str w`/`ldr w`.
    W,
    /// `str x`/`ldr x`.
    X,
    /// `ldrsb` into a w register.
    SbW,
    /// `ldrsb` into an x register.
    SbX,
    /// `ldrsh` into a w register.
    ShW,
    /// `ldrsh` into an x register.
    ShX,
    /// `ldrsw`.
    SwX,
}

impl Mem {
    /// log2 of the access size.
    fn scale(self) -> u32 {
        match self {
            Mem::B | Mem::SbW | Mem::SbX => 0,
            Mem::H | Mem::ShW | Mem::ShX => 1,
            Mem::W | Mem::SwX => 2,
            Mem::X => 3,
        }
    }
    /// size and opc fields of the load/store encodings, for a load.
    fn load_bits(self) -> (u32, u32) {
        match self {
            Mem::B => (0, 1),
            Mem::H => (1, 1),
            Mem::W => (2, 1),
            Mem::X => (3, 1),
            Mem::SbX => (0, 2),
            Mem::SbW => (0, 3),
            Mem::ShX => (1, 2),
            Mem::ShW => (1, 3),
            Mem::SwX => (2, 2),
        }
    }
}

/// A float width: single or double.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fw {
    S,
    D,
}

impl Fw {
    /// The `ftype` field of the scalar FP encodings.
    fn ftype(self) -> u32 {
        match self {
            Fw::S => 0,
            Fw::D => 1,
        }
    }
}

/// A place in the code a branch can be bound to later.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Fixup {
    at: usize,
    kind: BranchKind,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum BranchKind {
    /// `b`/`bl`: imm26 at bits 25:0.
    Imm26,
    /// `b.cond`, `cbz`/`cbnz`: imm19 at bits 23:5.
    Imm19,
    /// `tbz`/`tbnz`: imm14 at bits 18:5.
    Imm14,
}

fn sf(wide: bool) -> u32 {
    if wide { 1 << 31 } else { 0 }
}

fn r(x: R) -> u32 {
    x.0 as u32 & 31
}

fn v(x: V) -> u32 {
    x.0 as u32 & 31
}

/// The `N:immr:imms` field of a logical immediate for `value` in a
/// `width`-bit register, or `None` when `value` has no such encoding
/// (Arm ARM `DecodeBitMasks`, inverted): a run of ones, rotated, repeated
/// over an element of 2, 4, ..., 64 bits.
pub fn logical_imm(value: u64, wide: bool) -> Option<u32> {
    let width = if wide { 64 } else { 32 };
    let value = if wide { value } else { value & 0xffff_ffff };
    if value == 0 || (wide && value == u64::MAX) || (!wide && value == 0xffff_ffff) {
        return None;
    }
    let value = if wide { value } else { value | (value << 32) };
    // The smallest element size whose pattern repeats over 64 bits.
    let mut size = 64u32;
    while size > 2 {
        let half = size / 2;
        let mask = (1u64 << half) - 1;
        if (value & mask) != ((value >> half) & mask) {
            break;
        }
        size = half;
    }
    let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
    let elt = value & mask;
    let ones = elt.count_ones();
    // Rotate the element right until its ones sit at the bottom.
    let rot = (0..size).find(|&k| {
        let rotated = if k == 0 { elt } else { ((elt >> k) | (elt << (size - k))) & mask };
        rotated == (1u64 << ones) - 1
    })?;
    if size > width {
        return None;
    }
    let n = if size == 64 { 1 } else { 0 };
    let imms = ((!(size - 1) << 1) | (ones - 1)) & 0x3f;
    let immr = (size - rot) % size;
    Some((n << 12) | (immr << 6) | imms)
}

/// The code buffer.
pub struct Asm {
    pub buf: Vec<u8>,
}

impl Default for Asm {
    fn default() -> Self {
        Self::new()
    }
}

impl Asm {
    pub fn new() -> Asm {
        Asm { buf: Vec::new() }
    }

    pub fn pos(&self) -> usize {
        self.buf.len()
    }

    pub fn emit(&mut self, w: u32) {
        self.buf.extend_from_slice(&w.to_le_bytes());
    }

    fn word(&self, at: usize) -> u32 {
        u32::from_le_bytes([self.buf[at], self.buf[at + 1], self.buf[at + 2], self.buf[at + 3]])
    }

    fn set_word(&mut self, at: usize, w: u32) {
        self.buf[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }

    // ── Data processing, register ──────────────────────────────────────

    /// `add`/`sub`/`adds`/`subs` with a shifted register: `d = n op (m shift amount)`.
    fn addsub_shifted(&mut self, wide: bool, op: u32, s: u32, d: R, n: R, m: R, shift: Shift, amount: u32) {
        self.emit(sf(wide) | (op << 30) | (s << 29) | (0b01011 << 24) | ((shift as u32) << 22)
            | (r(m) << 16) | ((amount & 0x3f) << 10) | (r(n) << 5) | r(d));
    }

    pub fn add(&mut self, wide: bool, d: R, n: R, m: R) {
        self.addsub_shifted(wide, 0, 0, d, n, m, Shift::Lsl, 0);
    }
    pub fn add_shifted(&mut self, wide: bool, d: R, n: R, m: R, shift: Shift, amount: u32) {
        self.addsub_shifted(wide, 0, 0, d, n, m, shift, amount);
    }
    pub fn sub(&mut self, wide: bool, d: R, n: R, m: R) {
        self.addsub_shifted(wide, 1, 0, d, n, m, Shift::Lsl, 0);
    }
    pub fn subs(&mut self, wide: bool, d: R, n: R, m: R) {
        self.addsub_shifted(wide, 1, 1, d, n, m, Shift::Lsl, 0);
    }
    pub fn adds(&mut self, wide: bool, d: R, n: R, m: R) {
        self.addsub_shifted(wide, 0, 1, d, n, m, Shift::Lsl, 0);
    }
    /// `cmp n, m` (`subs zr, n, m`).
    pub fn cmp(&mut self, wide: bool, n: R, m: R) {
        self.subs(wide, ZR, n, m);
    }
    /// `neg d, m` (`sub d, zr, m`).
    pub fn neg(&mut self, wide: bool, d: R, m: R) {
        self.sub(wide, d, ZR, m);
    }

    /// `add`/`sub` with an extended register: `d = n op extend(m) << amount`.
    /// Register 31 is `sp` in `d` and `n` here.
    pub fn add_ext(&mut self, wide: bool, d: R, n: R, m: R, ext: Ext, amount: u32) {
        self.emit(sf(wide) | (0b0001011001 << 21) | (r(m) << 16) | ((ext as u32) << 13)
            | ((amount & 7) << 10) | (r(n) << 5) | r(d));
    }

    /// `add`/`sub`/`adds`/`subs` with a 12-bit immediate, optionally `lsl #12`.
    /// Register 31 is `sp` in `d` (unless setting flags) and in `n`.
    fn addsub_imm(&mut self, wide: bool, op: u32, s: u32, d: R, n: R, imm12: u32, lsl12: bool) {
        debug_assert!(imm12 < 4096);
        self.emit(sf(wide) | (op << 30) | (s << 29) | (0b100010 << 23) | ((lsl12 as u32) << 22)
            | ((imm12 & 0xfff) << 10) | (r(n) << 5) | r(d));
    }
    pub fn add_imm(&mut self, wide: bool, d: R, n: R, imm12: u32, lsl12: bool) {
        self.addsub_imm(wide, 0, 0, d, n, imm12, lsl12);
    }
    pub fn sub_imm(&mut self, wide: bool, d: R, n: R, imm12: u32, lsl12: bool) {
        self.addsub_imm(wide, 1, 0, d, n, imm12, lsl12);
    }
    pub fn subs_imm(&mut self, wide: bool, d: R, n: R, imm12: u32, lsl12: bool) {
        self.addsub_imm(wide, 1, 1, d, n, imm12, lsl12);
    }
    /// `cmp n, #imm12`.
    pub fn cmp_imm(&mut self, wide: bool, n: R, imm12: u32) {
        self.subs_imm(wide, ZR, n, imm12, false);
    }
    /// `cmn n, #imm12` (`adds zr, n, #imm12`).
    pub fn cmn_imm(&mut self, wide: bool, n: R, imm12: u32) {
        self.addsub_imm(wide, 0, 1, ZR, n, imm12, false);
    }
    /// `mov d, n` between `sp` and a register (`add d, n, #0`).
    pub fn mov_sp(&mut self, d: R, n: R) {
        self.add_imm(true, d, n, 0, false);
    }

    /// Logical with a shifted register. `opc`: 0 and, 1 orr, 2 eor, 3 ands;
    /// `inv` the N bit (bic, orn, eon, bics).
    fn logical(&mut self, wide: bool, opc: u32, inv: bool, d: R, n: R, m: R, shift: Shift, amount: u32) {
        self.emit(sf(wide) | (opc << 29) | (0b01010 << 24) | ((shift as u32) << 22)
            | ((inv as u32) << 21) | (r(m) << 16) | ((amount & 0x3f) << 10) | (r(n) << 5) | r(d));
    }
    pub fn and(&mut self, wide: bool, d: R, n: R, m: R) {
        self.logical(wide, 0, false, d, n, m, Shift::Lsl, 0);
    }
    pub fn orr(&mut self, wide: bool, d: R, n: R, m: R) {
        self.logical(wide, 1, false, d, n, m, Shift::Lsl, 0);
    }
    pub fn eor(&mut self, wide: bool, d: R, n: R, m: R) {
        self.logical(wide, 2, false, d, n, m, Shift::Lsl, 0);
    }
    pub fn ands(&mut self, wide: bool, d: R, n: R, m: R) {
        self.logical(wide, 3, false, d, n, m, Shift::Lsl, 0);
    }
    pub fn bic(&mut self, wide: bool, d: R, n: R, m: R) {
        self.logical(wide, 0, true, d, n, m, Shift::Lsl, 0);
    }
    pub fn orr_shifted(&mut self, wide: bool, d: R, n: R, m: R, shift: Shift, amount: u32) {
        self.logical(wide, 1, false, d, n, m, shift, amount);
    }
    /// `tst n, m` (`ands zr, n, m`).
    pub fn tst(&mut self, wide: bool, n: R, m: R) {
        self.ands(wide, ZR, n, m);
    }
    /// `mov d, m` (`orr d, zr, m`). A 32-bit move clears the upper half.
    pub fn mov(&mut self, wide: bool, d: R, m: R) {
        self.orr(wide, d, ZR, m);
    }
    /// `mvn d, m` (`orn d, zr, m`).
    pub fn mvn(&mut self, wide: bool, d: R, m: R) {
        self.logical(wide, 1, true, d, ZR, m, Shift::Lsl, 0);
    }

    /// Logical with an immediate in `logical_imm` form. `opc` as `logical`.
    fn logical_imm_raw(&mut self, wide: bool, opc: u32, d: R, n: R, nimm: u32) {
        self.emit(sf(wide) | (opc << 29) | (0b100100 << 23) | (nimm << 10) | (r(n) << 5) | r(d));
    }
    /// `and d, n, #imm`, if `imm` is a logical immediate. Register 31 is
    /// `sp` in `d`.
    pub fn and_imm(&mut self, wide: bool, d: R, n: R, imm: u64) -> bool {
        let Some(e) = logical_imm(imm, wide) else { return false };
        self.logical_imm_raw(wide, 0, d, n, e);
        true
    }
    pub fn orr_imm(&mut self, wide: bool, d: R, n: R, imm: u64) -> bool {
        let Some(e) = logical_imm(imm, wide) else { return false };
        self.logical_imm_raw(wide, 1, d, n, e);
        true
    }
    pub fn eor_imm(&mut self, wide: bool, d: R, n: R, imm: u64) -> bool {
        let Some(e) = logical_imm(imm, wide) else { return false };
        self.logical_imm_raw(wide, 2, d, n, e);
        true
    }
    pub fn tst_imm(&mut self, wide: bool, n: R, imm: u64) -> bool {
        let Some(e) = logical_imm(imm, wide) else { return false };
        self.logical_imm_raw(wide, 3, ZR, n, e);
        true
    }

    /// Two-source data processing: `opcode` 2 udiv, 3 sdiv, 8 lslv, 9 lsrv,
    /// 10 asrv, 11 rorv.
    fn dp2(&mut self, wide: bool, opcode: u32, d: R, n: R, m: R) {
        self.emit(sf(wide) | (0b0011010110 << 21) | (r(m) << 16) | (opcode << 10) | (r(n) << 5) | r(d));
    }
    pub fn udiv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 2, d, n, m);
    }
    pub fn sdiv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 3, d, n, m);
    }
    /// Shift by a register; the amount is taken modulo the width, as wasm wants.
    pub fn lslv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 8, d, n, m);
    }
    pub fn lsrv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 9, d, n, m);
    }
    pub fn asrv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 10, d, n, m);
    }
    pub fn rorv(&mut self, wide: bool, d: R, n: R, m: R) {
        self.dp2(wide, 11, d, n, m);
    }

    /// Three-source: `madd d, n, m, a` = a + n*m; `msub` = a - n*m.
    fn dp3(&mut self, wide: bool, o0: u32, d: R, n: R, m: R, a: R) {
        self.emit(sf(wide) | (0b0011011000 << 21) | (r(m) << 16) | (o0 << 15) | (r(a) << 10) | (r(n) << 5) | r(d));
    }
    pub fn madd(&mut self, wide: bool, d: R, n: R, m: R, a: R) {
        self.dp3(wide, 0, d, n, m, a);
    }
    pub fn msub(&mut self, wide: bool, d: R, n: R, m: R, a: R) {
        self.dp3(wide, 1, d, n, m, a);
    }
    /// `mul d, n, m` (`madd d, n, m, zr`).
    pub fn mul(&mut self, wide: bool, d: R, n: R, m: R) {
        self.madd(wide, d, n, m, ZR);
    }

    /// One-source data processing: `opcode` 0 rbit, 1 rev16, 2 rev(32), 4 clz, 5 cls.
    fn dp1(&mut self, wide: bool, opcode: u32, d: R, n: R) {
        self.emit(sf(wide) | (0b1011010110 << 21) | (opcode << 10) | (r(n) << 5) | r(d));
    }
    pub fn rbit(&mut self, wide: bool, d: R, n: R) {
        self.dp1(wide, 0, d, n);
    }
    pub fn clz(&mut self, wide: bool, d: R, n: R) {
        self.dp1(wide, 4, d, n);
    }

    /// Bitfield move. `opc` 0 sbfm, 2 ubfm.
    fn bfm(&mut self, wide: bool, opc: u32, d: R, n: R, immr: u32, imms: u32) {
        let nbit = if wide { 1 << 22 } else { 0 };
        self.emit(sf(wide) | (opc << 29) | (0b100110 << 23) | nbit | ((immr & 0x3f) << 16)
            | ((imms & 0x3f) << 10) | (r(n) << 5) | r(d));
    }
    /// `sxtb`/`sxth`/`sxtw`: sign extend the low `bits` (8, 16, 32).
    pub fn sxt(&mut self, wide: bool, d: R, n: R, bits: u32) {
        self.bfm(wide, 0, d, n, 0, bits - 1);
    }
    /// `uxtb`/`uxth`: zero extend the low `bits` (8, 16) into a w register.
    pub fn uxt(&mut self, d: R, n: R, bits: u32) {
        self.bfm(false, 2, d, n, 0, bits - 1);
    }
    /// `lsl d, n, #s` (immediate).
    pub fn lsl_imm(&mut self, wide: bool, d: R, n: R, s: u32) {
        let w = if wide { 64 } else { 32 };
        self.bfm(wide, 2, d, n, (w - s) % w, w - 1 - s);
    }
    /// `lsr d, n, #s` (immediate).
    pub fn lsr_imm(&mut self, wide: bool, d: R, n: R, s: u32) {
        let w = if wide { 64 } else { 32 };
        self.bfm(wide, 2, d, n, s, w - 1);
    }
    /// `asr d, n, #s` (immediate).
    pub fn asr_imm(&mut self, wide: bool, d: R, n: R, s: u32) {
        let w = if wide { 64 } else { 32 };
        self.bfm(wide, 0, d, n, s, w - 1);
    }

    /// Conditional select. `op`/`o2` pick csel, csinc, csinv, csneg.
    fn csel_raw(&mut self, wide: bool, op: u32, o2: u32, d: R, n: R, m: R, c: Cond) {
        self.emit(sf(wide) | (op << 30) | (0b11010100 << 21) | (r(m) << 16) | ((c as u32) << 12)
            | (o2 << 10) | (r(n) << 5) | r(d));
    }
    pub fn csel(&mut self, wide: bool, d: R, n: R, m: R, c: Cond) {
        self.csel_raw(wide, 0, 0, d, n, m, c);
    }
    pub fn csinc(&mut self, wide: bool, d: R, n: R, m: R, c: Cond) {
        self.csel_raw(wide, 0, 1, d, n, m, c);
    }
    /// `cset d, c` (`csinc d, zr, zr, !c`).
    pub fn cset(&mut self, wide: bool, d: R, c: Cond) {
        self.csinc(wide, d, ZR, ZR, c.not());
    }

    // ── Move wide ──────────────────────────────────────────────────────

    /// `opc` 0 movn, 2 movz, 3 movk; `hw` the 16-bit slot.
    fn movw(&mut self, wide: bool, opc: u32, d: R, imm16: u32, hw: u32) {
        self.emit(sf(wide) | (opc << 29) | (0b100101 << 23) | ((hw & 3) << 21) | ((imm16 & 0xffff) << 5) | r(d));
    }
    pub fn movz(&mut self, wide: bool, d: R, imm16: u32, hw: u32) {
        self.movw(wide, 2, d, imm16, hw);
    }
    pub fn movn(&mut self, wide: bool, d: R, imm16: u32, hw: u32) {
        self.movw(wide, 0, d, imm16, hw);
    }
    pub fn movk(&mut self, wide: bool, d: R, imm16: u32, hw: u32) {
        self.movw(wide, 3, d, imm16, hw);
    }

    /// Any constant into `d`: the shortest of movz/movn + movk, or a logical
    /// immediate.
    pub fn mov_imm(&mut self, wide: bool, d: R, value: u64) {
        let value = if wide { value } else { value & 0xffff_ffff };
        let halves = if wide { 4 } else { 2 };
        let ones_mask = if wide { u64::MAX } else { 0xffff_ffff };
        let part = |v: u64, k: u32| ((v >> (16 * k)) & 0xffff) as u32;
        let zeros = (0..halves).filter(|&k| part(value, k) == 0).count() as u32;
        let ones = (0..halves).filter(|&k| part(value, k) == 0xffff).count() as u32;
        // A logical immediate is one word: worth it when movz/movn needs more.
        if (halves - zeros).max(1) > 1 && (halves - ones).max(1) > 1 {
            if let Some(e) = logical_imm(value, wide) {
                self.logical_imm_raw(wide, 1, d, ZR, e);
                return;
            }
        }
        if ones > zeros {
            let inv = !value & ones_mask;
            let first = (0..halves).find(|&k| part(inv, k) != 0).unwrap_or(0);
            self.movn(wide, d, part(inv, first), first);
            for k in (first + 1)..halves {
                if part(value, k) != 0xffff {
                    self.movk(wide, d, part(value, k), k);
                }
            }
        } else {
            let first = (0..halves).find(|&k| part(value, k) != 0).unwrap_or(0);
            self.movz(wide, d, part(value, first), first);
            for k in (first + 1)..halves {
                if part(value, k) != 0 {
                    self.movk(wide, d, part(value, k), k);
                }
            }
        }
    }

    // ── Loads and stores ───────────────────────────────────────────────

    /// Load or store with an unsigned, size-scaled 12-bit offset.
    /// `off` must be a multiple of the access size and below 4096 of them.
    /// Register 31 is `sp` in `n`.
    pub fn ldr(&mut self, m: Mem, t: R, n: R, off: u32) {
        let (size, opc) = m.load_bits();
        let imm = off >> m.scale();
        debug_assert!(imm << m.scale() == off && imm < 4096);
        self.emit((size << 30) | (0b111001 << 24) | (opc << 22) | (imm << 10) | (r(n) << 5) | r(t));
    }
    pub fn str(&mut self, m: Mem, t: R, n: R, off: u32) {
        let size = m.load_bits().0;
        let imm = off >> m.scale();
        debug_assert!(imm << m.scale() == off && imm < 4096);
        self.emit((size << 30) | (0b111001 << 24) | (imm << 10) | (r(n) << 5) | r(t));
    }
    /// Whether `off` fits `ldr`/`str`'s scaled form for `m`.
    pub fn fits_scaled(m: Mem, off: i64) -> bool {
        off >= 0 && off & ((1 << m.scale()) - 1) == 0 && (off >> m.scale()) < 4096
    }
    /// `ldur`/`stur`: unscaled signed 9-bit offset.
    pub fn ldur(&mut self, m: Mem, t: R, n: R, off: i32) {
        let (size, opc) = m.load_bits();
        debug_assert!((-256..256).contains(&off));
        self.emit((size << 30) | (0b111000 << 24) | (opc << 22) | (((off as u32) & 0x1ff) << 12) | (r(n) << 5) | r(t));
    }
    pub fn stur(&mut self, m: Mem, t: R, n: R, off: i32) {
        let size = m.load_bits().0;
        debug_assert!((-256..256).contains(&off));
        self.emit((size << 30) | (0b111000 << 24) | (((off as u32) & 0x1ff) << 12) | (r(n) << 5) | r(t));
    }
    /// Register-offset load: `t = [n + ext(m) << (scaled ? size : 0)]`.
    pub fn ldr_reg(&mut self, mm: Mem, t: R, n: R, m: R, ext: Ext, scaled: bool) {
        let (size, opc) = mm.load_bits();
        self.emit((size << 30) | (0b111000 << 24) | (opc << 22) | (1 << 21) | (r(m) << 16)
            | ((ext as u32) << 13) | ((scaled as u32) << 12) | (0b10 << 10) | (r(n) << 5) | r(t));
    }
    pub fn str_reg(&mut self, mm: Mem, t: R, n: R, m: R, ext: Ext, scaled: bool) {
        let size = mm.load_bits().0;
        self.emit((size << 30) | (0b111000 << 24) | (1 << 21) | (r(m) << 16)
            | ((ext as u32) << 13) | ((scaled as u32) << 12) | (0b10 << 10) | (r(n) << 5) | r(t));
    }

    /// `stp`/`ldp` of x registers, pre-index (`[n, #off]!`), post-index
    /// (`[n], #off`) or signed offset. `off` a multiple of 8 in -512..=504.
    fn ldstp(&mut self, load: bool, mode: u32, t1: R, t2: R, n: R, off: i32) {
        debug_assert!(off % 8 == 0 && (-512..=504).contains(&off));
        let imm7 = ((off / 8) as u32) & 0x7f;
        self.emit((0b10 << 30) | (0b101 << 27) | (mode << 23) | ((load as u32) << 22) | (imm7 << 15)
            | (r(t2) << 10) | (r(n) << 5) | r(t1));
    }
    pub fn stp_pre(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(false, 0b011, t1, t2, n, off);
    }
    pub fn ldp_post(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(true, 0b001, t1, t2, n, off);
    }
    pub fn stp(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(false, 0b010, t1, t2, n, off);
    }
    pub fn ldp(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(true, 0b010, t1, t2, n, off);
    }

    // ── Branches ───────────────────────────────────────────────────────

    pub fn br(&mut self, n: R) {
        self.emit(0xd61f_0000 | (r(n) << 5));
    }
    pub fn blr(&mut self, n: R) {
        self.emit(0xd63f_0000 | (r(n) << 5));
    }
    pub fn ret(&mut self) {
        self.emit(0xd65f_0000 | (r(LR) << 5));
    }

    /// `b` / `bl` with a byte offset, a multiple of 4 within ±128 MiB.
    pub fn b_off(&mut self, link: bool, off: i64) {
        debug_assert!(off % 4 == 0 && (-(1 << 27)..(1 << 27)).contains(&off));
        self.emit(((link as u32) << 31) | (0b00101 << 26) | (((off >> 2) as u32) & 0x03ff_ffff));
    }
    /// `b.cond` with a byte offset within ±1 MiB.
    pub fn bcond_off(&mut self, c: Cond, off: i64) {
        debug_assert!(off % 4 == 0 && (-(1 << 20)..(1 << 20)).contains(&off));
        self.emit(0x5400_0000 | ((((off >> 2) as u32) & 0x7ffff) << 5) | (c as u32));
    }
    /// `cbz`/`cbnz` with a byte offset within ±1 MiB.
    pub fn cbz_off(&mut self, wide: bool, nz: bool, t: R, off: i64) {
        debug_assert!(off % 4 == 0 && (-(1 << 20)..(1 << 20)).contains(&off));
        self.emit(sf(wide) | (0b011010 << 25) | ((nz as u32) << 24) | ((((off >> 2) as u32) & 0x7ffff) << 5) | r(t));
    }

    /// A forward `b`, bound later.
    pub fn b(&mut self) -> Fixup {
        let at = self.pos();
        self.b_off(false, 0);
        Fixup { at, kind: BranchKind::Imm26 }
    }
    /// A `bl` whose target is resolved later (relocation).
    pub fn bl_blank(&mut self) -> usize {
        let at = self.pos();
        self.b_off(true, 0);
        at
    }
    /// A forward `b.cond`, bound later.
    pub fn bcond(&mut self, c: Cond) -> Fixup {
        let at = self.pos();
        self.bcond_off(c, 0);
        Fixup { at, kind: BranchKind::Imm19 }
    }
    /// A forward `cbz`/`cbnz`, bound later.
    pub fn cbz(&mut self, wide: bool, nz: bool, t: R) -> Fixup {
        let at = self.pos();
        self.cbz_off(wide, nz, t, 0);
        Fixup { at, kind: BranchKind::Imm19 }
    }
    /// A forward `tbz`/`tbnz` on bit `bit`, bound later.
    pub fn tbz(&mut self, nz: bool, t: R, bit: u32) -> Fixup {
        let at = self.pos();
        self.emit(((bit >> 5) << 31) | (0b011011 << 25) | ((nz as u32) << 24) | ((bit & 31) << 19) | r(t));
        Fixup { at, kind: BranchKind::Imm14 }
    }

    /// Point `f` at `target`. False if the distance does not fit.
    pub fn patch(&mut self, f: Fixup, target: usize) -> bool {
        let off = target as i64 - f.at as i64;
        let w = self.word(f.at);
        let w = match f.kind {
            BranchKind::Imm26 => {
                if !(-(1i64 << 27)..(1i64 << 27)).contains(&off) { return false; }
                (w & !0x03ff_ffff) | (((off >> 2) as u32) & 0x03ff_ffff)
            }
            BranchKind::Imm19 => {
                if !(-(1i64 << 20)..(1i64 << 20)).contains(&off) { return false; }
                (w & !(0x7ffff << 5)) | ((((off >> 2) as u32) & 0x7ffff) << 5)
            }
            BranchKind::Imm14 => {
                if !(-(1i64 << 15)..(1i64 << 15)).contains(&off) { return false; }
                (w & !(0x3fff << 5)) | ((((off >> 2) as u32) & 0x3fff) << 5)
            }
        };
        self.set_word(f.at, w);
        true
    }
    /// Bind `f` to the current position.
    pub fn bind(&mut self, f: Fixup) -> bool {
        let here = self.pos();
        self.patch(f, here)
    }
    /// Point the `bl`/`b` at `at` to `target` (a relocation).
    pub fn patch_b(&mut self, at: usize, target: usize) -> bool {
        self.patch(Fixup { at, kind: BranchKind::Imm26 }, target)
    }

    /// `adr d, #off`: a PC-relative address within ±1 MiB.
    pub fn adr_off(&mut self, d: R, off: i64) {
        debug_assert!((-(1 << 20)..(1 << 20)).contains(&off));
        let o = off as u32;
        self.emit(((o & 3) << 29) | (0b10000 << 24) | (((o >> 2) & 0x7ffff) << 5) | r(d));
    }
    /// An `adr` whose target is bound later.
    pub fn adr_blank(&mut self, d: R) -> usize {
        let at = self.pos();
        self.adr_off(d, 0);
        at
    }
    pub fn patch_adr(&mut self, at: usize, target: usize) -> bool {
        let off = target as i64 - at as i64;
        if !(-(1i64 << 20)..(1i64 << 20)).contains(&off) { return false; }
        let o = off as u32;
        let w = (self.word(at) & !((3 << 29) | (0x7ffff << 5))) | ((o & 3) << 29) | (((o >> 2) & 0x7ffff) << 5);
        self.set_word(at, w);
        true
    }

    // ── System ─────────────────────────────────────────────────────────

    pub fn brk(&mut self, imm16: u32) {
        self.emit(0xd420_0000 | ((imm16 & 0xffff) << 5));
    }
    pub fn nop(&mut self) {
        self.emit(0xd503_201f);
    }
    /// `csdb`: consumption of speculative data barrier.
    pub fn csdb(&mut self) {
        self.emit(0xd503_229f);
    }
    pub fn isb(&mut self) {
        self.emit(0xd503_3fdf);
    }

    // ── Floating point (scalar) ────────────────────────────────────────

    /// Two-source FP: `opcode` 0 fmul, 1 fdiv, 2 fadd, 3 fsub, 4 fmax, 5 fmin.
    fn fp2(&mut self, fw: Fw, opcode: u32, d: V, n: V, m: V) {
        self.emit((0b00011110 << 24) | (fw.ftype() << 22) | (1 << 21) | (v(m) << 16) | (opcode << 12)
            | (0b10 << 10) | (v(n) << 5) | v(d));
    }
    pub fn fmul(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 0, d, n, m); }
    pub fn fdiv(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 1, d, n, m); }
    pub fn fadd(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 2, d, n, m); }
    pub fn fsub(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 3, d, n, m); }
    pub fn fmax(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 4, d, n, m); }
    pub fn fmin(&mut self, fw: Fw, d: V, n: V, m: V) { self.fp2(fw, 5, d, n, m); }

    /// One-source FP: `opcode` 0 fmov, 1 fabs, 2 fneg, 3 fsqrt, 4 fcvt→s,
    /// 5 fcvt→d, 8 frintn, 9 frintp, 10 frintm, 11 frintz.
    fn fp1(&mut self, fw: Fw, opcode: u32, d: V, n: V) {
        self.emit((0b00011110 << 24) | (fw.ftype() << 22) | (1 << 21) | (opcode << 15) | (0b10000 << 10)
            | (v(n) << 5) | v(d));
    }
    pub fn fmov(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 0, d, n); }
    pub fn fabs(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 1, d, n); }
    pub fn fneg(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 2, d, n); }
    pub fn fsqrt(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 3, d, n); }
    /// Convert `n` of width `from` to the other width.
    pub fn fcvt(&mut self, from: Fw, d: V, n: V) {
        self.fp1(from, if from == Fw::D { 4 } else { 5 }, d, n);
    }
    pub fn frintn(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 8, d, n); }
    pub fn frintp(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 9, d, n); }
    pub fn frintm(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 10, d, n); }
    pub fn frintz(&mut self, fw: Fw, d: V, n: V) { self.fp1(fw, 11, d, n); }

    /// `fcmp n, m`.
    pub fn fcmp(&mut self, fw: Fw, n: V, m: V) {
        self.emit((0b00011110 << 24) | (fw.ftype() << 22) | (1 << 21) | (v(m) << 16) | (0b001000 << 10) | (v(n) << 5));
    }
    /// `fcsel d, n, m, c`.
    pub fn fcsel(&mut self, fw: Fw, d: V, n: V, m: V, c: Cond) {
        self.emit((0b00011110 << 24) | (fw.ftype() << 22) | (1 << 21) | (v(m) << 16) | ((c as u32) << 12)
            | (0b11 << 10) | (v(n) << 5) | v(d));
    }

    /// Conversion between FP and integer registers. `rmode`/`opcode` as the
    /// Arm ARM's "Conversion between floating-point and integer" table.
    fn fcvt_int(&mut self, wide: bool, fw: Fw, rmode: u32, opcode: u32, d: u32, n: u32) {
        self.emit(sf(wide) | (0b0011110 << 24) | (fw.ftype() << 22) | (1 << 21) | (rmode << 19) | (opcode << 16)
            | (n << 5) | d);
    }
    /// `scvtf d, n`: signed integer (`wide` = 64-bit) to float.
    pub fn scvtf(&mut self, wide: bool, fw: Fw, d: V, n: R) { self.fcvt_int(wide, fw, 0, 2, v(d), r(n)); }
    /// `ucvtf d, n`: unsigned integer to float.
    pub fn ucvtf(&mut self, wide: bool, fw: Fw, d: V, n: R) { self.fcvt_int(wide, fw, 0, 3, v(d), r(n)); }
    /// `fcvtzs d, n`: float to signed integer, toward zero, saturating.
    pub fn fcvtzs(&mut self, wide: bool, fw: Fw, d: R, n: V) { self.fcvt_int(wide, fw, 3, 0, r(d), v(n)); }
    /// `fcvtzu d, n`: float to unsigned integer, toward zero, saturating.
    pub fn fcvtzu(&mut self, wide: bool, fw: Fw, d: R, n: V) { self.fcvt_int(wide, fw, 3, 1, r(d), v(n)); }
    /// `fmov` of the bits from an integer register (`wide` with `Fw::D`).
    pub fn fmov_from_gpr(&mut self, fw: Fw, d: V, n: R) {
        self.fcvt_int(fw == Fw::D, fw, 0, 7, v(d), r(n));
    }
    /// `fmov` of the bits to an integer register.
    pub fn fmov_to_gpr(&mut self, fw: Fw, d: R, n: V) {
        self.fcvt_int(fw == Fw::D, fw, 0, 6, r(d), v(n));
    }

    /// FP load/store with an unsigned, size-scaled 12-bit offset.
    pub fn fldr(&mut self, fw: Fw, t: V, n: R, off: u32) {
        let (size, scale) = match fw { Fw::S => (2, 2), Fw::D => (3, 3) };
        let imm = off >> scale;
        debug_assert!(imm << scale == off && imm < 4096);
        self.emit((size << 30) | (0b111101 << 24) | (1 << 22) | (imm << 10) | (r(n) << 5) | v(t));
    }
    pub fn fstr(&mut self, fw: Fw, t: V, n: R, off: u32) {
        let (size, scale) = match fw { Fw::S => (2, 2), Fw::D => (3, 3) };
        let imm = off >> scale;
        debug_assert!(imm << scale == off && imm < 4096);
        self.emit((size << 30) | (0b111101 << 24) | (imm << 10) | (r(n) << 5) | v(t));
    }
    /// FP register-offset load/store: `[n + ext(m)]`, unscaled.
    pub fn fldr_reg(&mut self, fw: Fw, t: V, n: R, m: R, ext: Ext) {
        let size = match fw { Fw::S => 2, Fw::D => 3 };
        self.emit((size << 30) | (0b111100 << 24) | (1 << 22) | (1 << 21) | (r(m) << 16)
            | ((ext as u32) << 13) | (0b10 << 10) | (r(n) << 5) | v(t));
    }
    pub fn fstr_reg(&mut self, fw: Fw, t: V, n: R, m: R, ext: Ext) {
        let size = match fw { Fw::S => 2, Fw::D => 3 };
        self.emit((size << 30) | (0b111100 << 24) | (1 << 21) | (r(m) << 16)
            | ((ext as u32) << 13) | (0b10 << 10) | (r(n) << 5) | v(t));
    }

    /// `cnt v.8b, v.8b` then `addv b, v.8b`: popcount of the low 64 bits of
    /// `x`, the result in the low byte of `d`.
    pub fn cnt_8b(&mut self, d: V, n: V) {
        self.emit(0x0e20_5800 | (v(n) << 5) | v(d));
    }
    pub fn addv_8b(&mut self, d: V, n: V) {
        self.emit(0x0e31_b800 | (v(n) << 5) | v(d));
    }
}

#[cfg(test)]
#[path = "a64_tests.rs"]
mod a64_tests;
