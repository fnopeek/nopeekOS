//! wasm to AArch64, one function at a time, one pass. The same generator as
//! `codegen` (x86-64) in structure and in meaning: an operand-stack cache in
//! registers over one frame slot per value, local homes chosen by use count,
//! fuel charged per basic block and checked at loop heads and entries, the
//! stack limit checked per frame, traps grouped into stubs after the
//! function.
//!
//! Where the two processors differ, the difference is made explicit here:
//!
//! - Integer division never faults on A64: every `div`/`rem` tests its
//!   divisor for zero, and `div_s` tests for `INT_MIN / -1`, before it
//!   divides (`trap::DIVIDE_ERROR`). `rem_s` of `INT_MIN % -1` needs no test:
//!   `sdiv` answers `INT_MIN` and `msub` then gives 0.
//! - `fcvtzs`/`fcvtzu` saturate and map NaN to 0, which is `trunc_sat`
//!   exactly. The trapping truncations are not generated (as on x86), so the
//!   validator's feature set is the same for both.
//! - `fmin`/`fmax` follow IEEE 754-2008 like wasm: NaN in gives NaN, and
//!   `-0 < +0`.
//! - Linear memory is bounded by the same guard reservation as on x86: the
//!   address is `MEMBASE + zext(addr) + offset`, both terms u32, and an
//!   access outside the mapped part is a data abort the embedder sends to
//!   the trap path with `MEMORY_OUT_OF_BOUNDS`.
//!
//! Register plan (AAPCS64):
//!
//! | role                    | registers                         |
//! |-------------------------|-----------------------------------|
//! | VMCTX, MEMBASE, FUEL    | x28, x27, x26 (callee-saved)       |
//! | integer homes           | x19-x25 (callee-saved)             |
//! | float homes             | d8-d15 (low halves callee-saved)   |
//! | wasm arguments          | x1-x7, d0-d7, then the stack       |
//! | native arguments        | x0 = vmctx, x1-x7, d0-d7, stack    |
//! | results                 | x0, d0                             |
//! | operand cache           | x10-x15, d16-d23                   |
//! | operator scratch        | x0, x8, x9, d0-d2                  |
//! | value temp, address temp| x16, x17                           |
//! | frame                   | x29 = sp after the prologue, x30   |
//! | platform register x18   | never used                         |
//!
//! Every home is callee-saved, so neither a wasm call nor a native call
//! moves a local out of its register.
//!
//! Trap convention: the module's trap routine (`emit_trap_routine`) expects
//! the trap code in x0 and needs only x28 = vmctx; it stores the code at
//! `vmctx::TRAP_CODE`, puts back x29 = `TRAP_RBP` and sp = `TRAP_RSP`, and
//! branches to `TRAP_RESUME`. The resume point in the entry routine reads
//! the code back from `TRAP_CODE`, so an embedder may also skip the routine
//! and do those four steps itself without setting x0. A fault handler that
//! prefers the routine points the interrupted pc at `pf_entry`, which sets
//! x0 and falls into it.

use crate::a64::{self, Asm, Cond, Ext, Fixup, Fw, Mem, R, V};
use crate::codegen::{CompiledFunc, ModuleCtx, Outcome, Reloc, REFUEL};
use crate::trap;
use crate::vmctx;
use alloc::vec::Vec;
use wasmparser::{BlockType, Operator, ValType};

const A: R = a64::X0;
const B: R = a64::X8;
const C: R = a64::X9;
/// Materialises constants and moves between slots. Nothing holds a value in
/// it across an operator, and `call_indirect` keeps its target here while
/// the arguments are loaded, which neither spills nor writes it.
const T: R = a64::X16;
/// Address of a frame slot or a memory operand whose offset does not fit an
/// instruction. Never live beyond the instruction it serves.
const ADDR: R = a64::X17;
const FP: R = a64::FP;
const SP: R = a64::SP;
const ZR: R = a64::ZR;

const VMCTX: R = a64::X28;
const MEMBASE: R = a64::X27;
const FUEL: R = a64::X26;

const ARG_REGS: [R; 7] = [a64::X1, a64::X2, a64::X3, a64::X4, a64::X5, a64::X6, a64::X7];
const FARG_REGS: [V; 8] = [V(0), V(1), V(2), V(3), V(4), V(5), V(6), V(7)];

const HOMES: [R; 7] = [a64::X19, a64::X20, a64::X21, a64::X22, a64::X23, a64::X24, a64::X25];
const FHOMES: [V; 8] = [V(8), V(9), V(10), V(11), V(12), V(13), V(14), V(15)];

/// Disjoint from the argument registers, so loading arguments is a set of
/// independent moves without cycles.
const GPRS: [R; 6] = [a64::X10, a64::X11, a64::X12, a64::X13, a64::X14, a64::X15];
const FPRS: [V; 8] = [V(16), V(17), V(18), V(19), V(20), V(21), V(22), V(23)];

const FA: V = V(0);
const FB: V = V(1);
const FC: V = V(2);

/// As `codegen::LEAF_FRAME_MAX`.
const LEAF_FRAME_MAX: usize = 2048;
const HOME_MIN_USES: u64 = 4;

/// Bytes of the entry routine's frame: x29/x30, x19-x28, d8-d15.
const ENTRY_FRAME: i32 = 160;

fn wide(t: ValType) -> bool {
    matches!(t, ValType::I64 | ValType::F64)
}

fn is_float(t: ValType) -> bool {
    matches!(t, ValType::F32 | ValType::F64)
}

fn fw_of(t: ValType) -> Fw {
    if matches!(t, ValType::F64) { Fw::D } else { Fw::S }
}

fn fty(fw: Fw) -> ValType {
    if fw == Fw::D { ValType::F64 } else { ValType::F32 }
}

fn ity(w: bool) -> ValType {
    if w { ValType::I64 } else { ValType::I32 }
}

/// The access size of a frame slot holding a value of type `t`.
fn slot_mem(t: ValType) -> Mem {
    if wide(t) { Mem::X } else { Mem::W }
}

fn fmem(fw: Fw) -> Mem {
    if fw == Fw::D { Mem::X } else { Mem::W }
}

fn num_ty(t: ValType) -> bool {
    matches!(t, ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64)
}

// ── Module routines ───────────────────────────────────────────────────

/// Load or store `[base + off]` with any unsigned offset.
fn ld_at(a: &mut Asm, m: Mem, t: R, base: R, off: u32) {
    if Asm::fits_scaled(m, off as i64) {
        a.ldr(m, t, base, off);
    } else {
        a.mov_imm(true, ADDR, off as u64);
        a.ldr_reg(m, t, base, ADDR, Ext::Uxtx, false);
    }
}

fn st_at(a: &mut Asm, m: Mem, t: R, base: R, off: u32) {
    if Asm::fits_scaled(m, off as i64) {
        a.str(m, t, base, off);
    } else {
        a.mov_imm(true, ADDR, off as u64);
        a.str_reg(m, t, base, ADDR, Ext::Uxtx, false);
    }
}

fn fld_at(a: &mut Asm, fw: Fw, t: V, base: R, off: u32) {
    if Asm::fits_scaled(fmem(fw), off as i64) {
        a.fldr(fw, t, base, off);
    } else {
        a.mov_imm(true, ADDR, off as u64);
        a.fldr_reg(fw, t, base, ADDR, Ext::Uxtx);
    }
}

fn fst_at(a: &mut Asm, fw: Fw, t: V, base: R, off: u32) {
    if Asm::fits_scaled(fmem(fw), off as i64) {
        a.fstr(fw, t, base, off);
    } else {
        a.mov_imm(true, ADDR, off as u64);
        a.fstr_reg(fw, t, base, ADDR, Ext::Uxtx);
    }
}

fn vm_ld(a: &mut Asm, t: R, off: i32) {
    a.ldr(Mem::X, t, VMCTX, off as u32);
}

fn vm_st(a: &mut Asm, t: R, off: i32) {
    a.str(Mem::X, t, VMCTX, off as u32);
}

/// `sp -= n` / `sp += n` through `tmp` where `n` does not fit an immediate.
fn sp_sub(a: &mut Asm, n: u32, tmp: R) {
    if n < 4096 {
        a.sub_imm(true, SP, SP, n, false);
    } else {
        a.mov_imm(true, tmp, n as u64);
        a.sub_ext(true, SP, SP, tmp, Ext::Uxtx, 0);
    }
}

fn sp_add(a: &mut Asm, n: u32, tmp: R) {
    if n < 4096 {
        a.add_imm(true, SP, SP, n, false);
    } else {
        a.mov_imm(true, tmp, n as u64);
        a.add_ext(true, SP, SP, tmp, Ext::Uxtx, 0);
    }
}

/// Point the `b`/`bl` at `at` in `code` to `target`. False out of range.
pub(crate) fn patch_branch(code: &mut [u8], at: usize, target: usize) -> bool {
    let off = target as i64 - at as i64;
    if !(-(1i64 << 27)..(1i64 << 27)).contains(&off) {
        return false;
    }
    let w = u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]]);
    let w = (w & !0x03ff_ffff) | (((off >> 2) as u32) & 0x03ff_ffff);
    code[at..at + 4].copy_from_slice(&w.to_le_bytes());
    true
}

/// The trap routine: x0 = trap code, x28 = vmctx, nothing else assumed.
pub fn emit_trap_routine(a: &mut Asm) {
    vm_st(a, A, vmctx::TRAP_CODE);
    vm_ld(a, FP, vmctx::TRAP_RBP);
    vm_ld(a, T, vmctx::TRAP_RSP);
    a.mov_sp(SP, T);
    vm_ld(a, T, vmctx::TRAP_RESUME);
    a.br(T);
}

/// `movz w0, #reason; b <trap routine>`; the `b` at the returned offset
/// within the two words is patched by the caller.
pub fn emit_fault_entry(a: &mut Asm, reason: u32) -> usize {
    a.movz(false, A, reason, 0);
    let at = a.pos();
    a.b_off(false, 0);
    at
}

/// Where a fuel check lands when the counter ran out (`codegen::emit_refuel`).
/// Called with `bl`; the caller's own x30 is in its frame.
pub fn emit_refuel(a: &mut Asm) {
    vm_st(a, FUEL, vmctx::FUEL);
    vm_ld(a, T, vmctx::BUILTIN_FUEL);
    let none = a.cbz(true, false, T);
    a.stp_pre(FP, a64::LR, SP, -16);
    a.mov_sp(FP, SP);
    a.mov(true, A, VMCTX);
    a.blr(T);
    a.ldp_post(FP, a64::LR, SP, 16);
    vm_ld(a, FUEL, vmctx::FUEL);
    a.bind(none);
    a.ret();
}

/// `entry(vmctx, target, a, b, c) -> u32`, as `codegen::emit_entry`. Saves
/// every callee-saved register the module may use, because a trap leaves
/// without running any epilogue, and records where a trap resumes before
/// anything can trap.
pub fn emit_entry(a: &mut Asm) {
    a.stp_pre(FP, a64::LR, SP, -ENTRY_FRAME);
    a.mov_sp(FP, SP);
    a.stp(a64::X19, a64::X20, SP, 16);
    a.stp(a64::X21, a64::X22, SP, 32);
    a.stp(a64::X23, a64::X24, SP, 48);
    a.stp(a64::X25, a64::X26, SP, 64);
    a.stp(a64::X27, a64::X28, SP, 80);
    for k in 0..8u32 {
        a.fstr(Fw::D, V(8 + k as u8), SP, 96 + 8 * k);
    }
    a.mov(true, VMCTX, a64::X0);

    vm_st(a, FP, vmctx::TRAP_RBP);
    a.mov_sp(T, SP);
    vm_st(a, T, vmctx::TRAP_RSP);
    let adr_at = a.adr_blank(T);
    vm_st(a, T, vmctx::TRAP_RESUME);
    vm_st(a, ZR, vmctx::TRAP_CODE);

    vm_ld(a, MEMBASE, vmctx::MEM_BASE);
    vm_ld(a, FUEL, vmctx::FUEL);

    // The entry is checked against the stack limit once, as on x86.
    vm_ld(a, T, vmctx::STACK_LIMIT);
    a.mov_sp(ADDR, SP);
    a.cmp(true, ADDR, T);
    let room = a.bcond(Cond::Hs);
    a.movz(false, A, trap::STACK_EXHAUSTED, 0);
    vm_st(a, A, vmctx::TRAP_CODE);
    let refused = a.b();
    a.bind(room);

    a.mov(true, T, a64::X1);
    a.mov(false, a64::X1, a64::X2);
    a.mov(false, a64::X2, a64::X3);
    a.mov(false, a64::X3, a64::X4);
    a.blr(T);
    let done = a.b();

    // Every trap arrives here with x28, x26, x29 and sp restored and nothing
    // else; the code it left is the return value.
    a.bind(refused);
    let resume = a.pos();
    a.patch_adr(adr_at, resume);
    vm_ld(a, A, vmctx::TRAP_CODE);

    a.bind(done);
    vm_st(a, FUEL, vmctx::FUEL);
    for k in 0..8u32 {
        a.fldr(Fw::D, V(8 + k as u8), SP, 96 + 8 * k);
    }
    a.ldp(a64::X19, a64::X20, SP, 16);
    a.ldp(a64::X21, a64::X22, SP, 32);
    a.ldp(a64::X23, a64::X24, SP, 48);
    a.ldp(a64::X25, a64::X26, SP, 64);
    a.ldp(a64::X27, a64::X28, SP, 80);
    a.ldp_post(FP, a64::LR, SP, ENTRY_FRAME);
    a.ret();
}

// ── Function state ────────────────────────────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy)]
enum Kind {
    Func,
    Block,
    Loop,
    If,
}

struct Ctrl {
    kind: Kind,
    height: u32,
    branch_arity: u32,
    out_arity: u32,
    out_ty: ValType,
    start: usize,
    ends: Vec<Fixup>,
    /// (entry offset, table offset) of `br_table` entries waiting for the end.
    table_ends: Vec<(usize, usize)>,
    else_patch: Option<Fixup>,
    dead: bool,
}

#[derive(Copy, Clone)]
enum Place {
    Int(usize),
    Flt(usize),
    Stack(usize),
}

fn arg_places(params: &[ValType]) -> (Vec<Place>, usize) {
    let (mut int_n, mut flt_n, mut stack_n) = (0usize, 0usize, 0usize);
    let mut out = Vec::with_capacity(params.len());
    for t in params {
        if is_float(*t) {
            if flt_n < FARG_REGS.len() {
                out.push(Place::Flt(flt_n));
                flt_n += 1;
            } else {
                out.push(Place::Stack(stack_n));
                stack_n += 1;
            }
        } else if int_n < ARG_REGS.len() {
            out.push(Place::Int(int_n));
            int_n += 1;
        } else {
            out.push(Place::Stack(stack_n));
            stack_n += 1;
        }
    }
    (out, stack_n)
}

fn outgoing_bytes(n: usize) -> u32 {
    ((8 * n as u32) + 15) & !15
}

/// Where an operand-stack value is (see `codegen::Loc`). Integers live only
/// in `Gpr`, floats only in `Fpr`.
#[derive(Copy, Clone, PartialEq)]
enum Loc {
    Gpr(R),
    Fpr(V),
    Slot,
    Imm(i64),
    Local(u32),
    /// An i32 that is 1 when `Cond` holds on the flags the last compare
    /// left. Nothing between the compare and its consumer sets flags: fuel
    /// is charged with `sub`, constants with `movz`/`movk`/`orr`, spills
    /// are stores.
    Flags(Cond),
}

#[derive(Copy, Clone)]
struct Val {
    ty: ValType,
    loc: Loc,
    max: u32,
}

struct Ctx<'a> {
    asm: Asm,
    m: &'a ModuleCtx<'a>,
    gpr_used: [bool; 32],
    fpr_used: [bool; 32],
    local_types: Vec<ValType>,
    relocs: Vec<Reloc>,
    traps: Vec<(u32, Vec<Fixup>)>,
    trap_relocs: Vec<usize>,
    n_locals: u32,
    homed: [u32; HOMES.len()],
    n_homes: usize,
    fhomed: [u32; FHOMES.len()],
    n_fhomes: usize,
    stack: Vec<Val>,
    max_stack: u32,
    ctrl: Vec<Ctrl>,
    refuels: Vec<(Fixup, usize)>,
    fuel_pending: i64,
    reachable: bool,
    /// Conditional branches go as `b.!cc +8; b`, for functions too large for
    /// `b.cond`'s ±1 MiB.
    far: bool,
    /// A branch did not reach its target; the function is generated again
    /// with `far`, or refused.
    broken: bool,
}

impl<'a> Ctx<'a> {
    fn saved(&self) -> usize {
        self.n_homes + self.n_fhomes
    }
    fn home_save(&self, k: usize) -> u32 {
        16 + 8 * k as u32
    }
    fn fhome_save(&self, k: usize) -> u32 {
        16 + 8 * (self.n_homes + k) as u32
    }
    fn local(&self, i: u32) -> u32 {
        16 + 8 * (self.saved() as u32 + i)
    }
    fn slot(&self, d: u32) -> u32 {
        16 + 8 * (self.saved() as u32 + self.n_locals + d)
    }
    fn home(&self, i: u32) -> Option<R> {
        (0..self.n_homes).find(|&k| self.homed[k] == i).map(|k| HOMES[k])
    }
    fn fhome(&self, i: u32) -> Option<V> {
        (0..self.n_fhomes).find(|&k| self.fhomed[k] == i).map(|k| FHOMES[k])
    }

    fn ldf(&mut self, t: ValType, r: R, off: u32) {
        ld_at(&mut self.asm, slot_mem(t), r, FP, off);
    }
    fn stf(&mut self, t: ValType, r: R, off: u32) {
        st_at(&mut self.asm, slot_mem(t), r, FP, off);
    }
    fn fldf(&mut self, fw: Fw, x: V, off: u32) {
        fld_at(&mut self.asm, fw, x, FP, off);
    }
    fn fstf(&mut self, fw: Fw, x: V, off: u32) {
        fst_at(&mut self.asm, fw, x, FP, off);
    }

    fn mov_ty(&mut self, t: ValType, d: R, s: R) {
        if d != s || !wide(t) {
            self.asm.mov(wide(t), d, s);
        }
    }

    fn bind(&mut self, f: Fixup) {
        if !self.asm.bind(f) {
            self.broken = true;
        }
    }

    fn jcc(&mut self, cc: Cond) -> Fixup {
        if self.far {
            self.asm.bcond_off(cc.not(), 8);
            self.asm.b()
        } else {
            self.asm.bcond(cc)
        }
    }

    fn cbz(&mut self, w: bool, nz: bool, r: R) -> Fixup {
        if self.far {
            self.asm.cbz_off(w, !nz, r, 8);
            self.asm.b()
        } else {
            self.asm.cbz(w, nz, r)
        }
    }

    fn b_back(&mut self, target: usize) {
        let off = target as i64 - self.asm.pos() as i64;
        if !(-(1i64 << 27)..(1i64 << 27)).contains(&off) {
            self.broken = true;
            return;
        }
        self.asm.b_off(false, off);
    }

    fn jcc_back(&mut self, cc: Cond, target: usize) {
        let off = target as i64 - self.asm.pos() as i64;
        if (-(1i64 << 20)..(1i64 << 20)).contains(&off) {
            self.asm.bcond_off(cc, off);
        } else {
            self.asm.bcond_off(cc.not(), 8);
            self.b_back(target);
        }
    }

    fn call_routine(&mut self, target: u32) {
        let at = self.asm.bl_blank();
        self.relocs.push(Reloc { at, target });
    }

    fn depth(&self) -> u32 {
        self.stack.len() as u32
    }
    fn peek_ty(&self) -> Option<ValType> {
        self.stack.last().map(|v| v.ty)
    }
    fn note_depth(&mut self) {
        if self.depth() > self.max_stack {
            self.max_stack = self.depth();
        }
    }
    fn push(&mut self, ty: ValType, loc: Loc) {
        self.stack.push(Val { ty, loc, max: u32::MAX });
        self.note_depth();
    }
    fn push_reg_bounded(&mut self, r: R, max: u32) {
        self.stack.push(Val { ty: ValType::I32, loc: Loc::Gpr(r), max });
        self.note_depth();
    }

    fn settle_local(&mut self, idx: u32) {
        for i in 0..self.stack.len() {
            if self.stack[i].loc == Loc::Local(idx) {
                self.spill(i);
            }
        }
    }

    // --- registers ---

    fn alloc_gpr(&mut self) -> R {
        for _ in 0..2 {
            for &r in &GPRS {
                if !self.gpr_used[r.0 as usize] {
                    self.gpr_used[r.0 as usize] = true;
                    return r;
                }
            }
            if let Some(i) = self.stack.iter().position(|v| matches!(v.loc, Loc::Gpr(_))) {
                self.spill(i);
            }
        }
        A
    }

    fn alloc_fpr(&mut self) -> V {
        for _ in 0..2 {
            for &x in &FPRS {
                if !self.fpr_used[x.0 as usize] {
                    self.fpr_used[x.0 as usize] = true;
                    return x;
                }
            }
            if let Some(i) = self.stack.iter().position(|v| matches!(v.loc, Loc::Fpr(_))) {
                self.spill(i);
            }
        }
        FA
    }

    fn free_gpr(&mut self, r: R) {
        self.gpr_used[r.0 as usize] = false;
    }
    fn free_fpr(&mut self, x: V) {
        self.fpr_used[x.0 as usize] = false;
    }

    fn fconst(&mut self, fw: Fw, x: V, bits: u64, tmp: R) {
        if bits == 0 {
            self.asm.fmov_from_gpr(fw, x, ZR);
        } else {
            self.asm.mov_imm(fw == Fw::D, tmp, bits);
            self.asm.fmov_from_gpr(fw, x, tmp);
        }
    }

    /// Write the value at `i` to its slot. Materialises through `T`.
    fn spill(&mut self, i: usize) {
        let v = self.stack[i];
        let off = self.slot(i as u32);
        match v.loc {
            Loc::Slot => return,
            Loc::Imm(0) => self.stf(v.ty, ZR, off),
            Loc::Imm(c) => {
                self.asm.mov_imm(wide(v.ty), T, c as u64);
                self.stf(v.ty, T, off);
            }
            Loc::Local(idx) if let Some(h) = self.fhome(idx) => self.fstf(fw_of(v.ty), h, off),
            Loc::Local(idx) if let Some(h) = self.home(idx) => self.stf(v.ty, h, off),
            Loc::Local(idx) => {
                let src = self.local(idx);
                self.ldf(v.ty, T, src);
                self.stf(v.ty, T, off);
            }
            Loc::Gpr(r) => {
                self.stf(v.ty, r, off);
                self.free_gpr(r);
            }
            Loc::Fpr(x) => {
                self.fstf(fw_of(v.ty), x, off);
                self.free_fpr(x);
            }
            Loc::Flags(cc) => {
                self.asm.cset(false, T, cc);
                self.stf(ValType::I32, T, off);
            }
        }
        self.stack[i].loc = Loc::Slot;
    }

    fn push_flags(&mut self, cc: Cond) {
        self.push(ValType::I32, Loc::Flags(cc));
    }

    fn settle_flags(&mut self) {
        let Some(&Val { loc: Loc::Flags(cc), .. }) = self.stack.last() else { return };
        let i = self.stack.len() - 1;
        let r = self.alloc_gpr();
        self.asm.cset(false, r, cc);
        self.stack[i].loc = Loc::Gpr(r);
    }

    fn pop_cond(&mut self) -> Cond {
        if let Some(Loc::Flags(cc)) = self.stack.last().map(|v| v.loc) {
            self.stack.pop();
            return cc;
        }
        self.pop_to(A);
        self.asm.cmp_imm(false, A, 0);
        Cond::Ne
    }

    fn spill_all(&mut self) {
        for i in 0..self.stack.len() {
            self.spill(i);
        }
    }

    fn spill_below(&mut self, keep: usize) {
        let n = self.stack.len().saturating_sub(keep);
        for i in 0..n {
            self.spill(i);
        }
    }

    fn spill_for_branch(&mut self, relative_depth: u32) {
        let carries = self
            .ctrl
            .len()
            .checked_sub(1 + relative_depth as usize)
            .is_some_and(|i| self.ctrl[i].branch_arity == 1);
        self.spill_below(if carries { 1 } else { 0 });
    }

    fn truncate_to(&mut self, h: u32) {
        while self.stack.len() > h as usize {
            match self.stack.pop().map(|v| v.loc) {
                Some(Loc::Gpr(r)) => self.free_gpr(r),
                Some(Loc::Fpr(x)) => self.free_fpr(x),
                _ => {}
            }
        }
    }

    // --- the operand stack ---

    fn push_from_ty(&mut self, r: R, t: ValType) {
        let d = self.alloc_gpr();
        self.mov_ty(t, d, r);
        self.push(t, Loc::Gpr(d));
    }

    fn fpush_from(&mut self, x: V, t: ValType) {
        let d = self.alloc_fpr();
        if d != x {
            self.asm.fmov(fw_of(t), d, x);
        }
        self.push(t, Loc::Fpr(d));
    }

    fn pop_to_pool(&mut self) -> R {
        if let Some(Val { loc: Loc::Gpr(r), .. }) = self.stack.last().copied() {
            self.stack.pop();
            return r;
        }
        let d = self.alloc_gpr();
        self.pop_to(d);
        d
    }

    fn fpop_to_pool(&mut self) -> V {
        if let Some(Val { loc: Loc::Fpr(x), .. }) = self.stack.last().copied() {
            self.stack.pop();
            return x;
        }
        let d = self.alloc_fpr();
        self.fpop_to(d);
        d
    }

    /// The value in `v` (at depth `d` if it is in its slot) into `r`.
    fn read_to(&mut self, v: Val, d: u32, r: R) {
        match v.loc {
            Loc::Gpr(x) => self.mov_ty(v.ty, r, x),
            Loc::Fpr(x) => self.asm.fmov_to_gpr(fw_of(v.ty), r, x),
            Loc::Slot => {
                let off = self.slot(d);
                self.ldf(v.ty, r, off);
            }
            Loc::Imm(c) => self.asm.mov_imm(wide(v.ty), r, c as u64),
            Loc::Local(idx) if let Some(h) = self.fhome(idx) => self.asm.fmov_to_gpr(fw_of(v.ty), r, h),
            Loc::Local(idx) if let Some(h) = self.home(idx) => self.mov_ty(v.ty, r, h),
            Loc::Local(idx) => {
                let off = self.local(idx);
                self.ldf(v.ty, r, off);
            }
            Loc::Flags(cc) => self.asm.cset(false, r, cc),
        }
    }

    fn fread_to(&mut self, v: Val, d: u32, x: V) {
        let fw = fw_of(v.ty);
        match v.loc {
            Loc::Fpr(y) => {
                if x != y {
                    self.asm.fmov(fw, x, y);
                }
            }
            Loc::Gpr(r) => self.asm.fmov_from_gpr(fw, x, r),
            Loc::Slot => {
                let off = self.slot(d);
                self.fldf(fw, x, off);
            }
            Loc::Local(idx) if let Some(h) = self.fhome(idx) => self.asm.fmov(fw, x, h),
            Loc::Local(idx) if let Some(h) = self.home(idx) => self.asm.fmov_from_gpr(fw, x, h),
            Loc::Local(idx) => {
                let off = self.local(idx);
                self.fldf(fw, x, off);
            }
            Loc::Imm(c) => self.fconst(fw, x, c as u64, T),
            Loc::Flags(cc) => {
                self.asm.cset(false, T, cc);
                self.asm.fmov_from_gpr(Fw::S, x, T);
            }
        }
    }

    fn pop_to(&mut self, r: R) -> Option<ValType> {
        let v = self.stack.pop()?;
        let d = self.depth();
        self.read_to(v, d, r);
        match v.loc {
            Loc::Gpr(x) => self.free_gpr(x),
            Loc::Fpr(x) => self.free_fpr(x),
            _ => {}
        }
        Some(v.ty)
    }

    fn fpop_to(&mut self, x: V) -> Option<ValType> {
        let v = self.stack.pop()?;
        let d = self.depth();
        self.fread_to(v, d, x);
        match v.loc {
            Loc::Gpr(r) => self.free_gpr(r),
            Loc::Fpr(y) => self.free_fpr(y),
            _ => {}
        }
        Some(v.ty)
    }

    fn peek_to(&mut self, r: R) {
        let Some(v) = self.stack.last().copied() else { return };
        let d = self.depth() - 1;
        self.read_to(v, d, r);
    }

    fn fpeek_to(&mut self, x: V) {
        let Some(v) = self.stack.last().copied() else { return };
        let d = self.depth() - 1;
        self.fread_to(v, d, x);
    }

    /// A float operand to be read only: a cache register or a home as it is,
    /// anything else loaded into `scratch`. True: a cache register now
    /// belongs to the caller.
    fn fpop_in_place(&mut self, scratch: V) -> (V, bool) {
        match self.stack.last().map(|v| v.loc) {
            Some(Loc::Fpr(x)) => {
                self.stack.pop();
                (x, true)
            }
            Some(Loc::Local(i)) if let Some(h) = self.fhome(i) => {
                self.stack.pop();
                (h, false)
            }
            _ => {
                self.fpop_to(scratch);
                (scratch, false)
            }
        }
    }

    fn peek_result(&mut self) {
        if is_float(self.peek_ty().unwrap_or(ValType::I32)) {
            self.fpeek_to(FA);
        } else {
            self.peek_to(A);
        }
    }

    fn pop_result(&mut self) {
        if is_float(self.peek_ty().unwrap_or(ValType::I32)) {
            self.fpop_to(FA);
        } else {
            self.pop_to(A);
        }
    }

    // --- fuel and traps ---

    /// Charge what is pending with `sub`, which leaves the flags alone.
    fn fuel_flush(&mut self) {
        if self.fuel_pending == 0 {
            return;
        }
        let n = self.fuel_pending.clamp(0, i32::MAX as i64) as u32;
        if n < 1 << 12 {
            self.asm.sub_imm(true, FUEL, FUEL, n, false);
        } else if n < 1 << 24 {
            self.asm.sub_imm(true, FUEL, FUEL, n >> 12, true);
            if n & 0xfff != 0 {
                self.asm.sub_imm(true, FUEL, FUEL, n & 0xfff, false);
            }
        } else {
            self.asm.mov_imm(true, ADDR, n as u64);
            self.asm.sub(true, FUEL, FUEL, ADDR);
        }
        self.fuel_pending = 0;
    }

    /// As `codegen`: a budget of exactly zero is spent.
    fn fuel_check(&mut self) {
        self.asm.cmp_imm(true, FUEL, 0);
        let p = self.jcc(Cond::Le);
        let resume = self.asm.pos();
        self.refuels.push((p, resume));
    }

    fn emit_refuels(&mut self) {
        let refuels = core::mem::take(&mut self.refuels);
        for (p, resume) in refuels {
            self.bind(p);
            self.call_routine(REFUEL);
            self.asm.cmp_imm(true, FUEL, 0);
            self.trap_if(Cond::Le, trap::OUT_OF_FUEL);
            self.b_back(resume);
        }
    }

    fn trap_patch(&mut self, code: u32, p: Fixup) {
        if let Some(e) = self.traps.iter_mut().find(|(c, _)| *c == code) {
            e.1.push(p);
        } else {
            self.traps.push((code, alloc::vec![p]));
        }
    }

    fn trap_if(&mut self, cc: Cond, code: u32) {
        let p = self.jcc(cc);
        self.trap_patch(code, p);
    }

    fn trap_now(&mut self, code: u32) {
        let p = self.asm.b();
        self.trap_patch(code, p);
    }

    /// `cmp n, #imm` for any constant, through `B` when it does not fit.
    fn cmp_imm_any(&mut self, w: bool, n: R, c: i64) {
        let c = if w { c } else { c as i32 as i64 };
        if (0..4096).contains(&c) {
            self.asm.cmp_imm(w, n, c as u32);
        } else if (-4095..0).contains(&c) {
            self.asm.cmn_imm(w, n, (-c) as u32);
        } else {
            self.asm.mov_imm(w, ADDR, c as u64);
            self.asm.cmp(w, n, ADDR);
        }
    }
}

fn block_out(ty: BlockType) -> Result<(u32, ValType), &'static str> {
    match ty {
        BlockType::Empty => Ok((0, ValType::I32)),
        BlockType::Type(t) if num_ty(t) => Ok((1, t)),
        BlockType::Type(_) => Err("blocktype-value"),
        BlockType::FuncType(_) => Err("blocktype-multivalue"),
    }
}

/// As `codegen::choose_homes`, for seven integer and eight float homes.
fn choose_homes(local_types: &[ValType], ops: &[Operator<'_>]) -> ([u32; 7], usize, [u32; 8], usize) {
    let mut homed = [0u32; 7];
    let mut fhomed = [0u32; 8];
    if local_types.is_empty() {
        return (homed, 0, fhomed, 0);
    }
    let mut score = alloc::vec![0u64; local_types.len()];
    let mut loop_at: Vec<u32> = Vec::new();
    let mut depth = 0u32;
    for op in ops {
        let idx = match op {
            Operator::Block { .. } | Operator::If { .. } => {
                depth += 1;
                continue;
            }
            Operator::Loop { .. } => {
                depth += 1;
                loop_at.push(depth);
                continue;
            }
            Operator::End => {
                if loop_at.last() == Some(&depth) {
                    loop_at.pop();
                }
                depth = depth.saturating_sub(1);
                continue;
            }
            Operator::LocalGet { local_index }
            | Operator::LocalSet { local_index }
            | Operator::LocalTee { local_index } => *local_index,
            _ => continue,
        };
        if let Some(s) = score.get_mut(idx as usize) {
            *s = s.saturating_add(1 << (3 * loop_at.len().min(6)));
        }
    }
    let n = best(&score, local_types, false, &mut homed);
    let fnum = best(&score, local_types, true, &mut fhomed);
    (homed, n, fhomed, fnum)
}

fn best(score: &[u64], local_types: &[ValType], float: bool, out: &mut [u32]) -> usize {
    let mut n = 0;
    for i in 0..local_types.len() {
        if is_float(local_types[i]) != float || score[i] < HOME_MIN_USES {
            continue;
        }
        let at = (0..n).find(|&k| score[i] > score[out[k] as usize]).unwrap_or(n);
        if at < out.len() {
            for k in (at + 1..out.len()).rev() {
                out[k] = out[k - 1];
            }
            out[at] = i as u32;
            n = (n + 1).min(out.len());
        }
    }
    n
}

/// Generate one function. A function too large for `b.cond`'s range is
/// generated again with long conditional branches.
pub fn compile_func(
    params: &[ValType],
    results: &[ValType],
    local_decls: &[(u32, ValType)],
    ops: &[Operator<'_>],
    m: &ModuleCtx<'_>,
) -> Outcome {
    match compile_with(params, results, local_decls, ops, m, false) {
        Outcome::Unsupported("branch-range") => compile_with(params, results, local_decls, ops, m, true),
        o => o,
    }
}

fn compile_with(
    params: &[ValType],
    results: &[ValType],
    local_decls: &[(u32, ValType)],
    ops: &[Operator<'_>],
    m: &ModuleCtx<'_>,
    far: bool,
) -> Outcome {
    if !params.iter().copied().all(num_ty) {
        return Outcome::Unsupported("param-type");
    }
    if results.len() > 1 || !results.iter().copied().all(num_ty) {
        return Outcome::Unsupported("result-type");
    }
    if !local_decls.iter().all(|(_, t)| num_ty(*t)) {
        return Outcome::Unsupported("local-type");
    }

    let n_declared: u32 = local_decls.iter().map(|(c, _)| *c).sum();
    let n_locals = params.len() as u32 + n_declared;
    let out_arity = results.len() as u32;

    let mut local_types: Vec<ValType> = params.to_vec();
    for (count, t) in local_decls {
        for _ in 0..*count {
            local_types.push(*t);
        }
    }

    let mut asm = Asm::new();
    asm.buf.reserve(ops.len() * 16);
    let mut f = Ctx {
        asm,
        m,
        gpr_used: [false; 32],
        fpr_used: [false; 32],
        local_types,
        relocs: Vec::new(),
        traps: Vec::new(),
        trap_relocs: Vec::new(),
        n_locals,
        homed: [0; 7],
        n_homes: 0,
        fhomed: [0; 8],
        n_fhomes: 0,
        stack: Vec::new(),
        max_stack: 0,
        ctrl: Vec::new(),
        refuels: Vec::new(),
        fuel_pending: 0,
        reachable: true,
        far,
        broken: false,
    };

    f.ctrl.push(Ctrl {
        kind: Kind::Func,
        height: 0,
        branch_arity: out_arity,
        out_arity,
        out_ty: results.first().copied().unwrap_or(ValType::I32),
        start: 0,
        ends: Vec::new(),
        table_ends: Vec::new(),
        else_patch: None,
        dead: false,
    });

    (f.homed, f.n_homes, f.fhomed, f.n_fhomes) = choose_homes(&f.local_types, ops);

    // Prologue. The frame size is patched into the first two words at the
    // end. The frame is reserved and checked against the limit before
    // anything is written to it: a store below the limit could land in
    // whatever lies below the stack.
    let frame_patch = f.asm.pos();
    f.asm.movz(true, T, 0, 0);
    f.asm.movk(true, T, 0, 1);
    f.asm.sub_ext(true, SP, SP, T, Ext::Uxtx, 0);

    let leaf = !ops
        .iter()
        .any(|o| matches!(o, Operator::Call { .. } | Operator::CallIndirect { .. } | Operator::Loop { .. }));
    let frame_bound = 8 * (HOMES.len() + FHOMES.len() + n_locals as usize + ops.len() + 2) + 16;
    let small_leaf = leaf && frame_bound <= LEAF_FRAME_MAX;
    if !small_leaf {
        f.asm.mov_sp(A, SP);
        vm_ld(&mut f.asm, C, vmctx::STACK_LIMIT);
        f.asm.cmp(true, A, C);
        f.trap_if(Cond::Lo, trap::STACK_EXHAUSTED);
    }
    f.asm.stp(FP, a64::LR, SP, 0);
    f.asm.mov_sp(FP, SP);

    for k in 0..f.n_homes {
        let off = f.home_save(k);
        st_at(&mut f.asm, Mem::X, HOMES[k], FP, off);
    }
    for k in 0..f.n_fhomes {
        let off = f.fhome_save(k);
        fst_at(&mut f.asm, Fw::D, FHOMES[k], FP, off);
    }

    let (places, n_stack_params) = arg_places(params);
    if n_stack_params > 0 {
        // The caller's sp, where the stack arguments are; `T` still holds
        // the frame size.
        f.asm.add(true, T, FP, T);
    }
    for (i, t) in params.iter().enumerate() {
        let t = *t;
        let off = f.local(i as u32);
        if let Some(h) = f.fhome(i as u32) {
            match places[i] {
                Place::Flt(k) => f.asm.fmov(fw_of(t), h, FARG_REGS[k]),
                Place::Stack(j) => fld_at(&mut f.asm, fw_of(t), h, T, 8 * j as u32),
                Place::Int(_) => {}
            }
            continue;
        }
        if let Some(h) = f.home(i as u32) {
            match places[i] {
                Place::Int(k) => f.mov_ty(t, h, ARG_REGS[k]),
                Place::Stack(j) => ld_at(&mut f.asm, slot_mem(t), h, T, 8 * j as u32),
                Place::Flt(_) => {}
            }
            continue;
        }
        match places[i] {
            Place::Int(k) => f.stf(t, ARG_REGS[k], off),
            Place::Flt(k) => f.fstf(fw_of(t), FARG_REGS[k], off),
            Place::Stack(j) => {
                ld_at(&mut f.asm, slot_mem(t), A, T, 8 * j as u32);
                f.stf(t, A, off);
            }
        }
    }
    if n_declared > 0 {
        for i in params.len() as u32..n_locals {
            if let Some(h) = f.fhome(i) {
                f.asm.fmov_from_gpr(Fw::D, h, ZR);
                continue;
            }
            match f.home(i) {
                Some(h) => f.asm.mov(true, h, ZR),
                None => {
                    let off = f.local(i);
                    f.stf(ValType::I64, ZR, off);
                }
            }
        }
    }

    if !leaf {
        f.fuel_check();
    }

    for op in ops {
        if let Err(name) = emit(&mut f, op) {
            return Outcome::Unsupported(name);
        }
    }
    if !f.ctrl.is_empty() {
        return Outcome::Unsupported("unclosed-frame");
    }

    f.fuel_flush();

    let slots = f.saved() as u32 + n_locals + f.max_stack.max(1);
    let frame = (16 + 8 * slots + 15) & !15;

    // Epilogue. The result, if any, is in x0 or d0.
    for k in 0..f.n_homes {
        let off = f.home_save(k);
        ld_at(&mut f.asm, Mem::X, HOMES[k], FP, off);
    }
    for k in 0..f.n_fhomes {
        let off = f.fhome_save(k);
        fld_at(&mut f.asm, Fw::D, FHOMES[k], FP, off);
    }
    f.asm.mov_sp(SP, FP);
    f.asm.ldp(FP, a64::LR, SP, 0);
    sp_add(&mut f.asm, frame, T);
    f.asm.ret();

    f.emit_refuels();

    let traps = core::mem::take(&mut f.traps);
    for (code, patches) in traps {
        for p in patches {
            f.bind(p);
        }
        f.asm.movz(false, A, code, 0);
        let at = f.asm.pos();
        f.asm.b_off(false, 0);
        f.trap_relocs.push(at);
    }

    let mut words = Asm::new();
    words.movz(true, T, frame & 0xffff, 0);
    words.movk(true, T, frame >> 16, 1);
    f.asm.buf[frame_patch..frame_patch + 8].copy_from_slice(&words.buf);

    if f.broken {
        return Outcome::Unsupported("branch-range");
    }
    let max_stack = f.max_stack;
    Outcome::Done(CompiledFunc {
        code: core::mem::take(&mut f.asm.buf),
        relocs: core::mem::take(&mut f.relocs),
        trap_relocs: core::mem::take(&mut f.trap_relocs),
        frame,
        max_stack,
    })
}

#[derive(Copy, Clone)]
enum Alu {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Mul,
}

enum Sh {
    Shl,
    ShrU,
    ShrS,
    Rotl,
    Rotr,
}

#[derive(Copy, Clone)]
enum FCmp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

fn emit(f: &mut Ctx, op: &Operator<'_>) -> Result<(), &'static str> {
    use Operator::*;

    // Fuel is charged exactly as in `codegen::emit`.
    if f.reachable && !matches!(op, Block { .. } | Loop { .. } | Else | End | Nop) {
        f.fuel_pending += 1;
    }
    if !matches!(op, BrIf { .. } | If { .. } | I32Eqz) {
        f.settle_flags();
    }

    match op {
        Block { blockty } => {
            let (out, out_ty) = block_out(*blockty)?;
            let dead = !f.reachable;
            let height = f.depth();
            f.ctrl.push(Ctrl {
                kind: Kind::Block,
                height,
                branch_arity: out,
                out_arity: out,
                out_ty,
                start: 0,
                ends: Vec::new(),
                table_ends: Vec::new(),
                else_patch: None,
                dead,
            });
            return Ok(());
        }
        Loop { blockty } => {
            f.fuel_flush();
            let (out, out_ty) = block_out(*blockty)?;
            let dead = !f.reachable;
            if !dead {
                f.spill_all();
            }
            let head = f.asm.pos();
            if !dead {
                f.fuel_check();
            }
            let height = f.depth();
            f.ctrl.push(Ctrl {
                kind: Kind::Loop,
                height,
                branch_arity: 0,
                out_arity: out,
                out_ty,
                start: head,
                ends: Vec::new(),
                table_ends: Vec::new(),
                else_patch: None,
                dead,
            });
            return Ok(());
        }
        If { blockty } => {
            f.fuel_flush();
            let (out, out_ty) = block_out(*blockty)?;
            let dead = !f.reachable;
            let else_patch = if dead {
                None
            } else {
                let cc = f.pop_cond();
                f.spill_all();
                Some(f.jcc(cc.not()))
            };
            let height = f.depth();
            f.ctrl.push(Ctrl {
                kind: Kind::If,
                height,
                branch_arity: out,
                out_arity: out,
                out_ty,
                start: 0,
                ends: Vec::new(),
                table_ends: Vec::new(),
                else_patch,
                dead,
            });
            return Ok(());
        }
        Else => {
            f.fuel_flush();
            let top = f.ctrl.len() - 1;
            if f.ctrl[top].kind != Kind::If {
                return Err("else-without-if");
            }
            if f.ctrl[top].dead {
                return Ok(());
            }
            if f.reachable {
                if f.ctrl[top].out_arity == 1 {
                    f.pop_result();
                }
                f.spill_all();
                let p = f.asm.b();
                f.ctrl[top].ends.push(p);
            }
            let Some(ep) = f.ctrl[top].else_patch.take() else {
                return Err("double-else");
            };
            f.bind(ep);
            f.reachable = true;
            let h = f.ctrl[top].height;
            f.truncate_to(h);
            return Ok(());
        }
        End => {
            f.fuel_flush();
            if f.reachable {
                if f.ctrl.last().is_some_and(|c| c.out_arity == 1) {
                    f.pop_result();
                }
                f.spill_all();
            }
            let Some(mut fr) = f.ctrl.pop() else {
                return Err("end-without-frame");
            };
            if !fr.dead {
                if let Some(ep) = fr.else_patch.take() {
                    f.bind(ep);
                    f.reachable = true;
                }
                let branched = !fr.ends.is_empty() || !fr.table_ends.is_empty();
                while let Some(p) = fr.ends.pop() {
                    f.bind(p);
                }
                let here = f.asm.pos();
                for (entry, table) in core::mem::take(&mut fr.table_ends) {
                    let rel = (here as i64 - table as i64) as i32;
                    f.asm.buf[entry..entry + 4].copy_from_slice(&rel.to_le_bytes());
                }
                f.reachable |= branched;
            } else {
                f.reachable = false;
            }
            f.truncate_to(fr.height);
            if fr.out_arity == 1 && fr.kind != Kind::Func {
                if !f.reachable {
                    f.push(fr.out_ty, Loc::Slot);
                } else if is_float(fr.out_ty) {
                    f.fpush_from(FA, fr.out_ty);
                } else {
                    f.push_from_ty(A, fr.out_ty);
                }
            }
            return Ok(());
        }
        Br { relative_depth } => {
            f.fuel_flush();
            if f.reachable {
                f.spill_for_branch(*relative_depth);
                branch_to(f, *relative_depth)?;
                f.reachable = false;
            }
            return Ok(());
        }
        Return => {
            f.fuel_flush();
            if f.reachable {
                let d = f.ctrl.len() as u32 - 1;
                f.spill_for_branch(d);
                branch_to(f, d)?;
                f.reachable = false;
            }
            return Ok(());
        }
        BrTable { targets } => {
            f.fuel_flush();
            if f.reachable {
                f.spill_below(1);
                br_table(f, targets)?;
                f.reachable = false;
            }
            return Ok(());
        }
        BrIf { relative_depth } => {
            f.fuel_flush();
            if f.reachable {
                br_if(f, *relative_depth)?;
            }
            return Ok(());
        }
        Unreachable => {
            if f.reachable {
                f.fuel_flush();
                f.trap_now(trap::UNREACHABLE);
                f.reachable = false;
            }
            return Ok(());
        }
        _ => {}
    }

    if !f.reachable {
        return Ok(());
    }

    match op {
        Nop => {}
        Drop => {
            let d = f.depth();
            f.truncate_to(d.saturating_sub(1));
        }

        LocalGet { local_index } => {
            let t = *f.local_types.get(*local_index as usize).ok_or("local-index")?;
            f.push(t, Loc::Local(*local_index));
        }
        LocalSet { local_index } if let Some(h) = f.fhome(*local_index) => {
            f.settle_local(*local_index);
            f.fpop_to(h).ok_or("stack-empty")?;
        }
        LocalSet { local_index } if let Some(h) = f.home(*local_index) => {
            f.settle_local(*local_index);
            f.pop_to(h).ok_or("stack-empty")?;
        }
        LocalSet { local_index } => {
            f.settle_local(*local_index);
            let off = f.local(*local_index);
            let v = *f.stack.last().ok_or("stack-empty")?;
            match v.loc {
                Loc::Gpr(r) => {
                    f.stack.pop();
                    f.stf(v.ty, r, off);
                    f.free_gpr(r);
                }
                Loc::Fpr(x) => {
                    f.stack.pop();
                    f.fstf(fw_of(v.ty), x, off);
                    f.free_fpr(x);
                }
                Loc::Imm(0) => {
                    f.stack.pop();
                    f.stf(v.ty, ZR, off);
                }
                _ if is_float(v.ty) => {
                    f.fpop_to(FA);
                    f.fstf(fw_of(v.ty), FA, off);
                }
                _ => {
                    f.pop_to(A);
                    f.stf(v.ty, A, off);
                }
            }
        }
        LocalTee { local_index } if let Some(h) = f.fhome(*local_index) => {
            f.settle_local(*local_index);
            f.peek_ty().ok_or("stack-empty")?;
            f.fpeek_to(h);
        }
        LocalTee { local_index } if let Some(h) = f.home(*local_index) => {
            f.settle_local(*local_index);
            f.peek_ty().ok_or("stack-empty")?;
            f.peek_to(h);
        }
        LocalTee { local_index } => {
            f.settle_local(*local_index);
            let off = f.local(*local_index);
            let v = *f.stack.last().ok_or("stack-empty")?;
            match v.loc {
                Loc::Gpr(r) => f.stf(v.ty, r, off),
                Loc::Fpr(x) => f.fstf(fw_of(v.ty), x, off),
                _ if is_float(v.ty) => {
                    f.fpeek_to(FA);
                    f.fstf(fw_of(v.ty), FA, off);
                }
                _ => {
                    f.peek_to(A);
                    f.stf(v.ty, A, off);
                }
            }
        }

        GlobalGet { global_index } => {
            let t = match f.m.globals.get(*global_index as usize) {
                Some((t, _)) if num_ty(*t) => *t,
                Some(_) => return Err("global-type"),
                None => return Err("global-index"),
            };
            vm_ld(&mut f.asm, B, vmctx::GLOBALS);
            let off = (vmctx::GLOBAL_STRIDE as u32) * *global_index;
            if is_float(t) {
                fld_at(&mut f.asm, fw_of(t), FA, B, off);
                f.fpush_from(FA, t);
            } else {
                ld_at(&mut f.asm, slot_mem(t), A, B, off);
                f.push_from_ty(A, t);
            }
        }
        GlobalSet { global_index } => {
            let t = match f.m.globals.get(*global_index as usize) {
                Some((t, true)) if num_ty(*t) => *t,
                Some((t, false)) if num_ty(*t) => return Err("global-immutable"),
                Some(_) => return Err("global-type"),
                None => return Err("global-index"),
            };
            let off = (vmctx::GLOBAL_STRIDE as u32) * *global_index;
            if is_float(t) {
                f.fpop_to(FA).ok_or("stack-empty")?;
                vm_ld(&mut f.asm, B, vmctx::GLOBALS);
                fst_at(&mut f.asm, fw_of(t), FA, B, off);
            } else {
                f.pop_to(A).ok_or("stack-empty")?;
                vm_ld(&mut f.asm, B, vmctx::GLOBALS);
                st_at(&mut f.asm, slot_mem(t), A, B, off);
            }
        }

        I32Const { value } => f.push(ValType::I32, Loc::Imm(*value as i64)),
        I64Const { value } => f.push(ValType::I64, Loc::Imm(*value)),

        I32Add => bin(f, false, Alu::Add),
        I32Sub => bin(f, false, Alu::Sub),
        I32Mul => bin(f, false, Alu::Mul),
        I32And => bin(f, false, Alu::And),
        I32Or => bin(f, false, Alu::Or),
        I32Xor => bin(f, false, Alu::Xor),
        I64Add => bin(f, true, Alu::Add),
        I64Sub => bin(f, true, Alu::Sub),
        I64Mul => bin(f, true, Alu::Mul),
        I64And => bin(f, true, Alu::And),
        I64Or => bin(f, true, Alu::Or),
        I64Xor => bin(f, true, Alu::Xor),

        I32Shl => shift(f, false, Sh::Shl),
        I32ShrU => shift(f, false, Sh::ShrU),
        I32ShrS => shift(f, false, Sh::ShrS),
        I32Rotl => shift(f, false, Sh::Rotl),
        I32Rotr => shift(f, false, Sh::Rotr),
        I64Shl => shift(f, true, Sh::Shl),
        I64ShrU => shift(f, true, Sh::ShrU),
        I64ShrS => shift(f, true, Sh::ShrS),
        I64Rotl => shift(f, true, Sh::Rotl),
        I64Rotr => shift(f, true, Sh::Rotr),

        I32Eq => cmp(f, false, Cond::Eq),
        I32Ne => cmp(f, false, Cond::Ne),
        I32LtS => cmp(f, false, Cond::Lt),
        I32LtU => cmp(f, false, Cond::Lo),
        I32GtS => cmp(f, false, Cond::Gt),
        I32GtU => cmp(f, false, Cond::Hi),
        I32LeS => cmp(f, false, Cond::Le),
        I32LeU => cmp(f, false, Cond::Ls),
        I32GeS => cmp(f, false, Cond::Ge),
        I32GeU => cmp(f, false, Cond::Hs),
        I64Eq => cmp(f, true, Cond::Eq),
        I64Ne => cmp(f, true, Cond::Ne),
        I64LtS => cmp(f, true, Cond::Lt),
        I64LtU => cmp(f, true, Cond::Lo),
        I64GtS => cmp(f, true, Cond::Gt),
        I64GtU => cmp(f, true, Cond::Hi),
        I64LeS => cmp(f, true, Cond::Le),
        I64LeU => cmp(f, true, Cond::Ls),
        I64GeS => cmp(f, true, Cond::Ge),
        I64GeU => cmp(f, true, Cond::Hs),

        I32DivS => div_op(f, false, true, false),
        I32DivU => div_op(f, false, false, false),
        I32RemS => div_op(f, false, true, true),
        I32RemU => div_op(f, false, false, true),
        I64DivS => div_op(f, true, true, false),
        I64DivU => div_op(f, true, false, false),
        I64RemS => div_op(f, true, true, true),
        I64RemU => div_op(f, true, false, true),

        I32Eqz => eqz(f, false),
        I64Eqz => eqz(f, true),

        I32Clz | I64Clz => {
            let w = matches!(op, I64Clz);
            f.pop_to(A);
            f.asm.clz(w, A, A);
            f.push_from_ty(A, ity(w));
        }
        I32Ctz | I64Ctz => {
            let w = matches!(op, I64Ctz);
            f.pop_to(A);
            f.asm.rbit(w, A, A);
            f.asm.clz(w, A, A);
            f.push_from_ty(A, ity(w));
        }
        I32Popcnt | I64Popcnt => {
            let w = matches!(op, I64Popcnt);
            f.pop_to(A);
            f.asm.fmov_from_gpr(if w { Fw::D } else { Fw::S }, FC, A);
            f.asm.cnt_8b(FC, FC);
            f.asm.addv_8b(FC, FC);
            f.asm.fmov_to_gpr(Fw::S, A, FC);
            f.push_from_ty(A, ity(w));
        }

        I32WrapI64 => {
            f.pop_to(A);
            f.asm.mov(false, A, A);
            f.push_from_ty(A, ValType::I32);
        }
        I64ExtendI32U => {
            f.pop_to(A);
            f.asm.mov(false, A, A);
            f.push_from_ty(A, ValType::I64);
        }
        I64ExtendI32S | I64Extend32S => {
            f.pop_to(A);
            f.asm.sxt(true, A, A, 32);
            f.push_from_ty(A, ValType::I64);
        }
        I32Extend8S => {
            f.pop_to(A);
            f.asm.sxt(false, A, A, 8);
            f.push_from_ty(A, ValType::I32);
        }
        I32Extend16S => {
            f.pop_to(A);
            f.asm.sxt(false, A, A, 16);
            f.push_from_ty(A, ValType::I32);
        }
        I64Extend8S => {
            f.pop_to(A);
            f.asm.sxt(true, A, A, 8);
            f.push_from_ty(A, ValType::I64);
        }
        I64Extend16S => {
            f.pop_to(A);
            f.asm.sxt(true, A, A, 16);
            f.push_from_ty(A, ValType::I64);
        }

        F32Const { value } => f.push(ValType::F32, Loc::Imm(value.bits() as i64)),
        F64Const { value } => f.push(ValType::F64, Loc::Imm(value.bits() as i64)),

        F32Add => fbin(f, Fw::S, |a, w, d, n, m| a.fadd(w, d, n, m)),
        F32Sub => fbin(f, Fw::S, |a, w, d, n, m| a.fsub(w, d, n, m)),
        F32Mul => fbin(f, Fw::S, |a, w, d, n, m| a.fmul(w, d, n, m)),
        F32Div => fbin(f, Fw::S, |a, w, d, n, m| a.fdiv(w, d, n, m)),
        F64Add => fbin(f, Fw::D, |a, w, d, n, m| a.fadd(w, d, n, m)),
        F64Sub => fbin(f, Fw::D, |a, w, d, n, m| a.fsub(w, d, n, m)),
        F64Mul => fbin(f, Fw::D, |a, w, d, n, m| a.fmul(w, d, n, m)),
        F64Div => fbin(f, Fw::D, |a, w, d, n, m| a.fdiv(w, d, n, m)),
        F32Min => fbin(f, Fw::S, |a, w, d, n, m| a.fmin(w, d, n, m)),
        F32Max => fbin(f, Fw::S, |a, w, d, n, m| a.fmax(w, d, n, m)),
        F64Min => fbin(f, Fw::D, |a, w, d, n, m| a.fmin(w, d, n, m)),
        F64Max => fbin(f, Fw::D, |a, w, d, n, m| a.fmax(w, d, n, m)),

        F32Sqrt => funary(f, Fw::S, |a, w, x| a.fsqrt(w, x, x)),
        F64Sqrt => funary(f, Fw::D, |a, w, x| a.fsqrt(w, x, x)),
        F32Floor => funary(f, Fw::S, |a, w, x| a.frintm(w, x, x)),
        F64Floor => funary(f, Fw::D, |a, w, x| a.frintm(w, x, x)),
        F32Ceil => funary(f, Fw::S, |a, w, x| a.frintp(w, x, x)),
        F64Ceil => funary(f, Fw::D, |a, w, x| a.frintp(w, x, x)),
        F32Trunc => funary(f, Fw::S, |a, w, x| a.frintz(w, x, x)),
        F64Trunc => funary(f, Fw::D, |a, w, x| a.frintz(w, x, x)),
        // Ties to even, as wasm.
        F32Nearest => funary(f, Fw::S, |a, w, x| a.frintn(w, x, x)),
        F64Nearest => funary(f, Fw::D, |a, w, x| a.frintn(w, x, x)),
        // `fabs`/`fneg` change only the sign bit, NaNs included.
        F32Abs => funary(f, Fw::S, |a, w, x| a.fabs(w, x, x)),
        F64Abs => funary(f, Fw::D, |a, w, x| a.fabs(w, x, x)),
        F32Neg => funary(f, Fw::S, |a, w, x| a.fneg(w, x, x)),
        F64Neg => funary(f, Fw::D, |a, w, x| a.fneg(w, x, x)),
        F32Copysign => copysign(f, Fw::S),
        F64Copysign => copysign(f, Fw::D),

        F32Eq => fcmp(f, Fw::S, FCmp::Eq),
        F32Ne => fcmp(f, Fw::S, FCmp::Ne),
        F32Lt => fcmp(f, Fw::S, FCmp::Lt),
        F32Gt => fcmp(f, Fw::S, FCmp::Gt),
        F32Le => fcmp(f, Fw::S, FCmp::Le),
        F32Ge => fcmp(f, Fw::S, FCmp::Ge),
        F64Eq => fcmp(f, Fw::D, FCmp::Eq),
        F64Ne => fcmp(f, Fw::D, FCmp::Ne),
        F64Lt => fcmp(f, Fw::D, FCmp::Lt),
        F64Gt => fcmp(f, Fw::D, FCmp::Gt),
        F64Le => fcmp(f, Fw::D, FCmp::Le),
        F64Ge => fcmp(f, Fw::D, FCmp::Ge),

        F32DemoteF64 => {
            f.fpop_to(FA);
            f.asm.fcvt(Fw::D, FA, FA);
            f.fpush_from(FA, ValType::F32);
        }
        F64PromoteF32 => {
            f.fpop_to(FA);
            f.asm.fcvt(Fw::S, FA, FA);
            f.fpush_from(FA, ValType::F64);
        }

        F32ConvertI32S => int_to_f(f, Fw::S, false, true),
        F32ConvertI32U => int_to_f(f, Fw::S, false, false),
        F32ConvertI64S => int_to_f(f, Fw::S, true, true),
        F32ConvertI64U => int_to_f(f, Fw::S, true, false),
        F64ConvertI32S => int_to_f(f, Fw::D, false, true),
        F64ConvertI32U => int_to_f(f, Fw::D, false, false),
        F64ConvertI64S => int_to_f(f, Fw::D, true, true),
        F64ConvertI64U => int_to_f(f, Fw::D, true, false),

        // `fcvtz*` saturate and answer 0 for NaN: wasm's `trunc_sat`.
        I32TruncSatF32S => trunc_sat(f, Fw::S, false, true),
        I32TruncSatF32U => trunc_sat(f, Fw::S, false, false),
        I32TruncSatF64S => trunc_sat(f, Fw::D, false, true),
        I32TruncSatF64U => trunc_sat(f, Fw::D, false, false),
        I64TruncSatF32S => trunc_sat(f, Fw::S, true, true),
        I64TruncSatF32U => trunc_sat(f, Fw::S, true, false),
        I64TruncSatF64S => trunc_sat(f, Fw::D, true, true),
        I64TruncSatF64U => trunc_sat(f, Fw::D, true, false),

        I32ReinterpretF32 | I64ReinterpretF64 => {
            let fw = if matches!(op, I64ReinterpretF64) { Fw::D } else { Fw::S };
            f.fpop_to(FA);
            f.asm.fmov_to_gpr(fw, A, FA);
            f.push_from_ty(A, ity(fw == Fw::D));
        }
        F32ReinterpretI32 | F64ReinterpretI64 => {
            let fw = if matches!(op, F64ReinterpretI64) { Fw::D } else { Fw::S };
            f.pop_to(A);
            f.asm.fmov_from_gpr(fw, FA, A);
            f.fpush_from(FA, fty(fw));
        }

        MemoryGrow { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            f.spill_all();
            f.pop_to(a64::X1);
            f.asm.mov(true, a64::X0, VMCTX);
            vm_ld(&mut f.asm, T, vmctx::BUILTIN_GROW);
            f.asm.blr(T);
            // AAPCS64 leaves the upper half of a 32-bit result undefined.
            f.asm.mov(false, A, A);
            f.push_from_ty(A, ValType::I32);
        }
        MemorySize { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            vm_ld(&mut f.asm, A, vmctx::MEM_SIZE);
            f.asm.lsr_imm(true, A, A, 16);
            f.push_from_ty(A, ValType::I32);
        }

        // Bulk memory checks its whole range before the first byte moves,
        // in 64 bits, where `start + len` of two u32 cannot wrap.
        MemoryCopy { dst_mem, src_mem } => {
            if *dst_mem != 0 || *src_mem != 0 {
                return Err("multi-memory");
            }
            f.spill_all();
            bulk_fuel(f);
            f.pop_to(B); // len
            f.pop_to(C); // src
            f.pop_to(A); // dst
            bulk_bounds(f, A);
            bulk_bounds(f, C);
            memory_copy(f);
        }
        MemoryFill { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            f.spill_all();
            bulk_fuel(f);
            f.pop_to(B); // len
            f.pop_to(C); // the byte
            f.pop_to(A); // dst
            bulk_bounds(f, A);
            memory_fill(f);
        }

        Call { function_index } => {
            f.fuel_flush();
            call_direct(f, *function_index)?;
        }
        CallIndirect { type_index, table_index } => {
            f.fuel_flush();
            if *table_index != 0 {
                return Err("multi-table");
            }
            call_indirect(f, *type_index)?;
        }

        F32Load { memarg } => fload(f, memarg, Fw::S)?,
        F64Load { memarg } => fload(f, memarg, Fw::D)?,
        F32Store { memarg } => fstore(f, memarg, Fw::S)?,
        F64Store { memarg } => fstore(f, memarg, Fw::D)?,

        I32Load { memarg } => load(f, memarg, ValType::I32, Mem::W)?,
        I32Load8U { memarg } => load(f, memarg, ValType::I32, Mem::B)?,
        I32Load8S { memarg } => load(f, memarg, ValType::I32, Mem::SbW)?,
        I32Load16U { memarg } => load(f, memarg, ValType::I32, Mem::H)?,
        I32Load16S { memarg } => load(f, memarg, ValType::I32, Mem::ShW)?,
        I64Load { memarg } => load(f, memarg, ValType::I64, Mem::X)?,
        I64Load8U { memarg } => load(f, memarg, ValType::I64, Mem::B)?,
        I64Load16U { memarg } => load(f, memarg, ValType::I64, Mem::H)?,
        I64Load32U { memarg } => load(f, memarg, ValType::I64, Mem::W)?,
        I64Load8S { memarg } => load(f, memarg, ValType::I64, Mem::SbX)?,
        I64Load16S { memarg } => load(f, memarg, ValType::I64, Mem::ShX)?,
        I64Load32S { memarg } => load(f, memarg, ValType::I64, Mem::SwX)?,

        I32Store { memarg } | I64Store32 { memarg } => store(f, memarg, Mem::W)?,
        I32Store8 { memarg } | I64Store8 { memarg } => store(f, memarg, Mem::B)?,
        I32Store16 { memarg } | I64Store16 { memarg } => store(f, memarg, Mem::H)?,
        I64Store { memarg } => store(f, memarg, Mem::X)?,

        // The condition is tested once; nothing between the compare and the
        // select sets flags.
        Select => {
            f.pop_to(C);
            let Some(t) = f.peek_ty() else {
                return Err("select-empty");
            };
            if !num_ty(t) {
                return Err("select-type");
            }
            f.asm.cmp_imm(false, C, 0);
            if is_float(t) {
                f.fpop_to(FB);
                f.fpop_to(FA);
                f.asm.fcsel(fw_of(t), FA, FA, FB, Cond::Ne);
                f.fpush_from(FA, t);
            } else {
                f.pop_to(B);
                f.pop_to(A);
                f.asm.csel(wide(t), A, A, B, Cond::Ne);
                f.push_from_ty(A, t);
            }
        }

        other => return Err(crate::names::op_name(other)),
    }
    Ok(())
}

fn branch_to(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {
    let idx = f
        .ctrl
        .len()
        .checked_sub(1 + relative_depth as usize)
        .ok_or("br-depth")?;
    if f.ctrl[idx].branch_arity == 1 {
        f.peek_result();
    }
    if f.ctrl[idx].kind == Kind::Loop {
        let start = f.ctrl[idx].start;
        f.b_back(start);
    } else {
        let p = f.asm.b();
        f.ctrl[idx].ends.push(p);
    }
    Ok(())
}

/// Dispatch, then a stub per case that carries a value, then the table of
/// 32-bit offsets from the table to each target. As on x86, the index is
/// clamped with a constant mask before the table is read, whatever the
/// range check predicted.
fn br_table(f: &mut Ctx, t: &wasmparser::BrTable<'_>) -> Result<(), &'static str> {
    let n = t.len() as usize;
    let mut labels: Vec<u32> = Vec::with_capacity(n);
    for x in t.targets() {
        labels.push(x.map_err(|_| "brtable-read")?);
    }
    let default = t.default();
    let frame = |f: &Ctx, d: u32| f.ctrl.len().checked_sub(1 + d as usize).ok_or("br-depth");
    let default_idx = frame(f, default)?;

    let in_range = f.stack.last().is_some_and(|v| (v.max as u64) < n as u64);
    f.pop_to(B);
    let to_default_stub = if in_range {
        None
    } else if f.ctrl[default_idx].branch_arity == 0 {
        f.cmp_imm_any(false, B, n as i64);
        if f.ctrl[default_idx].kind == Kind::Loop {
            let start = f.ctrl[default_idx].start;
            f.jcc_back(Cond::Hs, start);
        } else {
            let p = f.jcc(Cond::Hs);
            f.ctrl[default_idx].ends.push(p);
        }
        None
    } else {
        f.cmp_imm_any(false, B, n as i64);
        Some(f.jcc(Cond::Hs))
    };
    let slots = n.max(1).next_power_of_two();
    if !in_range {
        if slots == 1 {
            f.asm.mov(false, B, ZR);
        } else {
            f.asm.and_imm(false, B, B, (slots - 1) as u64);
        }
    }

    let adr_at = f.asm.adr_blank(T);
    f.asm.ldr_reg(Mem::SwX, ADDR, T, B, Ext::Uxtw, true);
    f.asm.add(true, T, T, ADDR);
    f.asm.br(T);

    enum Dest {
        Code(usize),
        Label(usize),
    }
    let dest_of = |f: &mut Ctx, d: u32| -> Result<Dest, &'static str> {
        let idx = frame(f, d)?;
        if f.ctrl[idx].branch_arity == 0 {
            if f.ctrl[idx].kind == Kind::Loop {
                return Ok(Dest::Code(f.ctrl[idx].start));
            }
            return Ok(Dest::Label(idx));
        }
        let at = f.asm.pos();
        branch_to(f, d)?;
        Ok(Dest::Code(at))
    };
    let mut dests = Vec::with_capacity(n);
    for &d in &labels {
        dests.push(dest_of(f, d)?);
    }
    let dflt = match to_default_stub {
        Some(p) => {
            f.bind(p);
            let at = f.asm.pos();
            branch_to(f, default)?;
            Dest::Code(at)
        }
        None => dest_of(f, default)?,
    };

    let table = f.asm.pos();
    if !f.asm.patch_adr(adr_at, table) {
        f.broken = true;
    }
    let put = |f: &mut Ctx, d: &Dest| match *d {
        Dest::Code(at) => f.asm.emit((at as i64 - table as i64) as i32 as u32),
        Dest::Label(idx) => {
            let entry = f.asm.pos();
            f.asm.emit(0);
            f.ctrl[idx].table_ends.push((entry, table));
        }
    };
    for d in &dests {
        put(f, d);
    }
    for _ in n..slots {
        put(f, &dflt);
    }
    Ok(())
}

fn br_if(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {
    let idx = f
        .ctrl
        .len()
        .checked_sub(1 + relative_depth as usize)
        .ok_or("br-depth")?;
    let arity = f.ctrl[idx].branch_arity;
    let cc = f.pop_cond();
    f.spill_all();
    if arity == 0 {
        if f.ctrl[idx].kind == Kind::Loop {
            let start = f.ctrl[idx].start;
            f.jcc_back(cc, start);
        } else {
            let p = f.jcc(cc);
            f.ctrl[idx].ends.push(p);
        }
        return Ok(());
    }
    let skip = f.jcc(cc.not());
    branch_to(f, relative_depth)?;
    f.bind(skip);
    Ok(())
}

/// One unit per 64 bytes, then a check, as `codegen::bulk_fuel`.
fn bulk_fuel(f: &mut Ctx) {
    f.fuel_flush();
    f.peek_to(A);
    f.asm.lsr_imm(true, A, A, 6);
    f.asm.sub(true, FUEL, FUEL, A);
    f.fuel_check();
}

/// `start + len > memory size` traps; `start` is the zero-extended address
/// in `reg`, `len` in `B`.
fn bulk_bounds(f: &mut Ctx, reg: R) {
    vm_ld(&mut f.asm, T, vmctx::MEM_SIZE);
    f.asm.add(true, ADDR, reg, B);
    f.asm.cmp(true, ADDR, T);
    f.trap_if(Cond::Hi, trap::MEMORY_OUT_OF_BOUNDS);
}

/// `A` = dst, `C` = src, `B` = len, all checked. A memmove: backward only
/// when the destination lies inside `src..src+len`. Eight bytes at a time,
/// each read before its write, then single bytes.
fn memory_copy(f: &mut Ctx) {
    let a = &mut f.asm;
    a.add(true, A, MEMBASE, A);
    a.add(true, C, MEMBASE, C);
    a.sub(true, T, A, C);
    a.cmp(true, T, B);
    let forward = a.bcond(Cond::Hs);

    let loop8 = a.pos();
    a.cmp_imm(true, B, 8);
    let bytes = a.bcond(Cond::Lo);
    a.sub_imm(true, B, B, 8, false);
    a.ldr_reg(Mem::X, ADDR, C, B, Ext::Uxtx, false);
    a.str_reg(Mem::X, ADDR, A, B, Ext::Uxtx, false);
    let back = loop8 as i64 - a.pos() as i64;
    a.b_off(false, back);
    a.bind(bytes);
    let loop1 = a.pos();
    let done_b = a.cbz(true, false, B);
    a.sub_imm(true, B, B, 1, false);
    a.ldr_reg(Mem::B, ADDR, C, B, Ext::Uxtx, false);
    a.str_reg(Mem::B, ADDR, A, B, Ext::Uxtx, false);
    let back = loop1 as i64 - a.pos() as i64;
    a.b_off(false, back);

    a.bind(forward);
    let loop8 = a.pos();
    a.cmp_imm(true, B, 8);
    let bytes = a.bcond(Cond::Lo);
    a.ldr(Mem::X, ADDR, C, 0);
    a.str(Mem::X, ADDR, A, 0);
    a.add_imm(true, A, A, 8, false);
    a.add_imm(true, C, C, 8, false);
    a.sub_imm(true, B, B, 8, false);
    let back = loop8 as i64 - a.pos() as i64;
    a.b_off(false, back);
    a.bind(bytes);
    let loop1 = a.pos();
    let done_f = a.cbz(true, false, B);
    a.ldr(Mem::B, ADDR, C, 0);
    a.str(Mem::B, ADDR, A, 0);
    a.add_imm(true, A, A, 1, false);
    a.add_imm(true, C, C, 1, false);
    a.sub_imm(true, B, B, 1, false);
    let back = loop1 as i64 - a.pos() as i64;
    a.b_off(false, back);
    a.bind(done_b);
    a.bind(done_f);
}

/// `A` = dst, `C` = the byte, `B` = len, checked.
fn memory_fill(f: &mut Ctx) {
    let a = &mut f.asm;
    a.add(true, A, MEMBASE, A);
    a.and_imm(false, C, C, 0xff);
    a.mov_imm(true, T, 0x0101_0101_0101_0101);
    a.mul(true, C, C, T);
    let loop8 = a.pos();
    a.cmp_imm(true, B, 8);
    let bytes = a.bcond(Cond::Lo);
    a.str(Mem::X, C, A, 0);
    a.add_imm(true, A, A, 8, false);
    a.sub_imm(true, B, B, 8, false);
    let back = loop8 as i64 - a.pos() as i64;
    a.b_off(false, back);
    a.bind(bytes);
    let loop1 = a.pos();
    let done = a.cbz(true, false, B);
    a.str(Mem::B, C, A, 0);
    a.add_imm(true, A, A, 1, false);
    a.sub_imm(true, B, B, 1, false);
    let back = loop1 as i64 - a.pos() as i64;
    a.b_off(false, back);
    a.bind(done);
}

fn callee_shape<'a>(m: &'a ModuleCtx<'a>, type_index: u32) -> Result<(&'a [ValType], &'a [ValType]), &'static str> {
    let (p, r) = m.types.get(type_index as usize).ok_or("callee-type")?;
    if !p.iter().copied().all(num_ty) {
        return Err("callee-param-type");
    }
    if r.len() > 1 || !r.iter().copied().all(num_ty) {
        return Err("callee-result-type");
    }
    Ok((p, r))
}

/// Stack arguments must be in their slots; everything else goes straight to
/// its register.
fn spill_arg_conflicts(f: &mut Ctx, places: &[Place], base: usize) {
    for (i, p) in places.iter().enumerate() {
        if matches!(p, Place::Stack(_)) {
            f.spill(base + i);
        }
    }
}

/// Arguments into place. Writes only `sp`, `x0`, `x17` and the argument
/// registers: `T` survives it, which `call_indirect` needs. Returns the bytes
/// reserved below `sp`.
fn load_args(f: &mut Ctx, params: &[ValType], places: &[Place], n_stack: usize) -> u32 {
    let n = params.len();
    let base = f.depth() as usize - n;
    let bytes = outgoing_bytes(n_stack);
    if bytes > 0 {
        sp_sub(&mut f.asm, bytes, A);
        for (i, t) in params.iter().enumerate() {
            let Place::Stack(j) = places[i] else { continue };
            let off = f.slot((base + i) as u32);
            // A 32-bit load zero-extends: the whole slot is defined.
            f.ldf(*t, A, off);
            st_at(&mut f.asm, Mem::X, A, SP, 8 * j as u32);
        }
    }
    for (i, t) in params.iter().enumerate() {
        let t = *t;
        let v = f.stack[base + i];
        let off = f.slot((base + i) as u32);
        match places[i] {
            Place::Int(k) => {
                let dst = ARG_REGS[k];
                match v.loc {
                    Loc::Gpr(r) => f.mov_ty(t, dst, r),
                    Loc::Imm(c) => f.asm.mov_imm(wide(t), dst, c as u64),
                    Loc::Local(idx) if let Some(h) = f.home(idx) => f.mov_ty(t, dst, h),
                    Loc::Local(idx) => {
                        let src = f.local(idx);
                        f.ldf(t, dst, src);
                    }
                    Loc::Flags(cc) => f.asm.cset(false, dst, cc),
                    Loc::Fpr(x) => f.asm.fmov_to_gpr(fw_of(t), dst, x),
                    Loc::Slot => f.ldf(t, dst, off),
                }
            }
            Place::Flt(k) => {
                let dst = FARG_REGS[k];
                let fw = fw_of(t);
                match v.loc {
                    Loc::Fpr(x) => f.asm.fmov(fw, dst, x),
                    Loc::Local(idx) if let Some(h) = f.fhome(idx) => f.asm.fmov(fw, dst, h),
                    Loc::Local(idx) => {
                        let src = f.local(idx);
                        f.fldf(fw, dst, src);
                    }
                    Loc::Imm(c) => f.fconst(fw, dst, c as u64, A),
                    Loc::Gpr(r) => f.asm.fmov_from_gpr(fw, dst, r),
                    _ => f.fldf(fw, dst, off),
                }
            }
            Place::Stack(_) => {}
        }
    }
    bytes
}

fn after_call(f: &mut Ctx, n_args: usize, results: &[ValType], arg_bytes: u32) {
    if arg_bytes > 0 {
        sp_add(&mut f.asm, arg_bytes, ADDR);
    }
    let keep = f.depth().saturating_sub(n_args as u32);
    f.truncate_to(keep);
    if let Some(t) = results.first() {
        if is_float(*t) {
            f.fpush_from(FA, *t);
        } else {
            f.push_from_ty(A, *t);
        }
    }
}

fn call_direct(f: &mut Ctx, function_index: u32) -> Result<(), &'static str> {
    let ti = *f.m.func_type_of.get(function_index as usize).ok_or("callee-index")?;
    let (params, results) = callee_shape(f.m, ti)?;
    let n = params.len();
    let (places, n_stack) = arg_places(params);
    f.spill_below(n);
    let base = f.stack.len() - n;
    spill_arg_conflicts(f, &places, base);
    let arg_bytes = load_args(f, params, &places, n_stack);

    if function_index < f.m.n_imported {
        // A native function: the context in x0, the counter handed over and
        // taken back. Homes are callee-saved under AAPCS64 and need nothing.
        f.asm.mov(true, a64::X0, VMCTX);
        vm_st(&mut f.asm, FUEL, vmctx::FUEL);
        vm_ld(&mut f.asm, T, vmctx::HOST_FNS);
        ld_at(&mut f.asm, Mem::X, T, T, 8 * function_index);
        f.asm.blr(T);
        vm_ld(&mut f.asm, FUEL, vmctx::FUEL);
        // AAPCS64 leaves the upper half of a 32-bit result undefined.
        if results.first() == Some(&ValType::I32) {
            f.asm.mov(false, A, A);
        }
    } else {
        let at = f.asm.bl_blank();
        f.relocs.push(Reloc { at, target: function_index });
    }
    after_call(f, n, results, arg_bytes);
    Ok(())
}

/// The table index is the one bound checked in code; the mask keeps a
/// mispredicted index inside the padded table.
fn call_indirect(f: &mut Ctx, type_index: u32) -> Result<(), &'static str> {
    let (params, results) = callee_shape(f.m, type_index)?;
    let n = params.len();
    let want = *f.m.sig_id.get(type_index as usize).ok_or("callee-type")?;
    let (places, n_stack) = arg_places(params);
    f.spill_below(n + 1);
    f.pop_to(B);
    let base = f.stack.len() - n;
    spill_arg_conflicts(f, &places, base);

    f.cmp_imm_any(false, B, f.m.table_len as i64);
    f.trap_if(Cond::Hs, trap::TABLE_OUT_OF_BOUNDS);
    let mask = crate::table_slots(f.m.table_len) - 1;
    if mask == 0 {
        f.asm.mov(false, B, ZR);
    } else {
        f.asm.and_imm(false, B, B, mask as u64);
    }

    vm_ld(&mut f.asm, T, vmctx::TABLE_SIGS);
    f.asm.ldr_reg(Mem::W, T, T, B, Ext::Uxtw, true);
    f.cmp_imm_any(false, T, want as i64);
    f.trap_if(Cond::Ne, trap::BAD_SIGNATURE);

    vm_ld(&mut f.asm, T, vmctx::TABLE);
    f.asm.ldr_reg(Mem::X, T, T, B, Ext::Uxtw, true);

    let arg_bytes = load_args(f, params, &places, n_stack);
    f.asm.blr(T);
    after_call(f, n, results, arg_bytes);
    Ok(())
}

/// Where a memory operand is: `[MEMBASE + uxtw(B)]`, `[T + off]` or
/// `[T + ADDR]`, with `T = MEMBASE + uxtw(B)`. Both terms are u32, so the
/// address stays inside the reservation.
enum Addr {
    Index,
    Imm(u32),
    Reg,
}

fn address(f: &mut Ctx, memarg: &wasmparser::MemArg, m: Mem) -> Result<Addr, &'static str> {
    if memarg.memory != 0 {
        return Err("multi-memory");
    }
    let off = memarg.offset;
    if off > u32::MAX as u64 {
        return Err("offset>4G");
    }
    if off == 0 {
        return Ok(Addr::Index);
    }
    f.asm.add_ext(true, T, MEMBASE, B, Ext::Uxtw, 0);
    if Asm::fits_scaled(m, off as i64) {
        Ok(Addr::Imm(off as u32))
    } else {
        f.asm.mov_imm(true, ADDR, off);
        Ok(Addr::Reg)
    }
}

fn load(f: &mut Ctx, memarg: &wasmparser::MemArg, ty: ValType, m: Mem) -> Result<(), &'static str> {
    f.pop_to(B);
    let d = f.alloc_gpr();
    match address(f, memarg, m)? {
        Addr::Index => f.asm.ldr_reg(m, d, MEMBASE, B, Ext::Uxtw, false),
        Addr::Imm(o) => f.asm.ldr(m, d, T, o),
        Addr::Reg => f.asm.ldr_reg(m, d, T, ADDR, Ext::Uxtx, false),
    }
    f.push(ty, Loc::Gpr(d));
    Ok(())
}

fn store(f: &mut Ctx, memarg: &wasmparser::MemArg, m: Mem) -> Result<(), &'static str> {
    let (src, owned) = match f.stack.last().map(|v| v.loc) {
        Some(Loc::Gpr(r)) => {
            f.stack.pop();
            (r, true)
        }
        Some(Loc::Local(i)) if let Some(h) = f.home(i) => {
            f.stack.pop();
            (h, false)
        }
        Some(Loc::Imm(0)) => {
            f.stack.pop();
            (ZR, false)
        }
        _ => {
            f.pop_to(A);
            (A, false)
        }
    };
    f.pop_to(B);
    match address(f, memarg, m)? {
        Addr::Index => f.asm.str_reg(m, src, MEMBASE, B, Ext::Uxtw, false),
        Addr::Imm(o) => f.asm.str(m, src, T, o),
        Addr::Reg => f.asm.str_reg(m, src, T, ADDR, Ext::Uxtx, false),
    }
    if owned {
        f.free_gpr(src);
    }
    Ok(())
}

fn fload(f: &mut Ctx, memarg: &wasmparser::MemArg, fw: Fw) -> Result<(), &'static str> {
    f.pop_to(B);
    let d = f.alloc_fpr();
    match address(f, memarg, fmem(fw))? {
        Addr::Index => f.asm.fldr_reg(fw, d, MEMBASE, B, Ext::Uxtw),
        Addr::Imm(o) => f.asm.fldr(fw, d, T, o),
        Addr::Reg => f.asm.fldr_reg(fw, d, T, ADDR, Ext::Uxtx),
    }
    f.push(fty(fw), Loc::Fpr(d));
    Ok(())
}

fn fstore(f: &mut Ctx, memarg: &wasmparser::MemArg, fw: Fw) -> Result<(), &'static str> {
    let (src, owned) = f.fpop_in_place(FA);
    f.pop_to(B);
    match address(f, memarg, fmem(fw))? {
        Addr::Index => f.asm.fstr_reg(fw, src, MEMBASE, B, Ext::Uxtw),
        Addr::Imm(o) => f.asm.fstr(fw, src, T, o),
        Addr::Reg => f.asm.fstr_reg(fw, src, T, ADDR, Ext::Uxtx),
    }
    if owned {
        f.free_fpr(src);
    }
    Ok(())
}

fn fbin(f: &mut Ctx, fw: Fw, mut go: impl FnMut(&mut Asm, Fw, V, V, V)) {
    let (rhs, owned) = f.fpop_in_place(FB);
    let d = f.fpop_to_pool();
    go(&mut f.asm, fw, d, d, rhs);
    if owned {
        f.free_fpr(rhs);
    }
    f.push(fty(fw), Loc::Fpr(d));
}

fn funary(f: &mut Ctx, fw: Fw, mut go: impl FnMut(&mut Asm, Fw, V)) {
    let d = f.fpop_to_pool();
    go(&mut f.asm, fw, d);
    f.push(fty(fw), Loc::Fpr(d));
}

/// Bit work in general registers: the magnitude without its sign, or the
/// donor's sign. NaN payloads pass through.
fn copysign(f: &mut Ctx, fw: Fw) {
    let w = fw == Fw::D;
    f.fpop_to(FB);
    f.fpop_to(FA);
    f.asm.fmov_to_gpr(fw, A, FA);
    f.asm.fmov_to_gpr(fw, B, FB);
    let sign: u64 = if w { 1 << 63 } else { 1 << 31 };
    let mag = if w { !sign } else { (sign - 1) & 0xffff_ffff };
    f.asm.and_imm(w, A, A, mag);
    f.asm.and_imm(w, B, B, sign);
    f.asm.orr(w, A, A, B);
    f.asm.fmov_from_gpr(fw, FA, A);
    f.fpush_from(FA, fty(fw));
}

/// `fcmp` leaves NZCV = 0011 for unordered. The conditions are chosen so
/// that unordered is false for every comparison but `ne`: `mi` for less
/// than (`lt` would hold, N != V), `ls` for less or equal.
fn fcmp(f: &mut Ctx, fw: Fw, kind: FCmp) {
    let (r, ro) = f.fpop_in_place(FB);
    let (l, lo) = f.fpop_in_place(FA);
    f.asm.fcmp(fw, l, r);
    if ro {
        f.free_fpr(r);
    }
    if lo {
        f.free_fpr(l);
    }
    let cc = match kind {
        FCmp::Eq => Cond::Eq,
        FCmp::Ne => Cond::Ne,
        FCmp::Lt => Cond::Mi,
        FCmp::Gt => Cond::Gt,
        FCmp::Le => Cond::Ls,
        FCmp::Ge => Cond::Ge,
    };
    f.push_flags(cc);
}

fn int_to_f(f: &mut Ctx, fw: Fw, w: bool, signed: bool) {
    f.pop_to(A);
    if signed {
        f.asm.scvtf(w, fw, FA, A);
    } else {
        f.asm.ucvtf(w, fw, FA, A);
    }
    f.fpush_from(FA, fty(fw));
}

fn trunc_sat(f: &mut Ctx, fw: Fw, w: bool, signed: bool) {
    f.fpop_to(FA);
    if signed {
        f.asm.fcvtzs(w, fw, A, FA);
    } else {
        f.asm.fcvtzu(w, fw, A, FA);
    }
    f.push_from_ty(A, ity(w));
}

enum Rhs {
    Imm(i64),
    Reg(R),
    Owned(R),
}

fn take_rhs(f: &mut Ctx) -> Rhs {
    let Some(top) = f.stack.last().copied() else {
        return Rhs::Reg(B);
    };
    match top.loc {
        Loc::Imm(c) => {
            f.stack.pop();
            Rhs::Imm(c)
        }
        Loc::Local(idx) if let Some(h) = f.home(idx) => {
            f.stack.pop();
            Rhs::Reg(h)
        }
        Loc::Gpr(r) => {
            f.stack.pop();
            Rhs::Owned(r)
        }
        _ => {
            f.pop_to(B);
            Rhs::Reg(B)
        }
    }
}

fn release_rhs(f: &mut Ctx, rhs: &Rhs) {
    if let Rhs::Owned(r) = rhs {
        f.free_gpr(*r);
    }
}

fn alu_rr(a: &mut Asm, w: bool, op: Alu, d: R, n: R, m: R) {
    match op {
        Alu::Add => a.add(w, d, n, m),
        Alu::Sub => a.sub(w, d, n, m),
        Alu::And => a.and(w, d, n, m),
        Alu::Or => a.orr(w, d, n, m),
        Alu::Xor => a.eor(w, d, n, m),
        Alu::Mul => a.mul(w, d, n, m),
    }
}

/// `d = d op c` with the shortest encoding, else through `B`.
fn alu_imm(a: &mut Asm, w: bool, op: Alu, d: R, c: i64) {
    let c = if w { c } else { c as i32 as i64 };
    let bits = if w { c as u64 } else { c as u64 & 0xffff_ffff };
    let done = match op {
        Alu::Add | Alu::Sub => {
            let c = if matches!(op, Alu::Sub) { c.wrapping_neg() } else { c };
            if (0..4096).contains(&c) {
                a.add_imm(w, d, d, c as u32, false);
                true
            } else if (-4095..0).contains(&c) {
                a.sub_imm(w, d, d, (-c) as u32, false);
                true
            } else {
                false
            }
        }
        Alu::And => a.and_imm(w, d, d, bits),
        Alu::Or => a.orr_imm(w, d, d, bits),
        Alu::Xor => a.eor_imm(w, d, d, bits),
        Alu::Mul => false,
    };
    if !done {
        a.mov_imm(w, B, bits);
        alu_rr(a, w, op, d, d, B);
    }
}

fn bin(f: &mut Ctx, w: bool, op: Alu) {
    let rhs = take_rhs(f);
    let d = f.pop_to_pool();
    match rhs {
        Rhs::Imm(c) => alu_imm(&mut f.asm, w, op, d, c),
        Rhs::Reg(r) | Rhs::Owned(r) => alu_rr(&mut f.asm, w, op, d, d, r),
    }
    release_rhs(f, &rhs);
    match rhs {
        Rhs::Imm(c) if !w && matches!(op, Alu::And) => f.push_reg_bounded(d, c as u32),
        _ => f.push(ity(w), Loc::Gpr(d)),
    }
}

/// Register shifts take the count modulo the width, as wasm does.
fn shift(f: &mut Ctx, w: bool, sh: Sh) {
    let width = if w { 64 } else { 32 };
    let imm = match f.stack.last().map(|v| v.loc) {
        Some(Loc::Imm(c)) => {
            f.stack.pop();
            Some((c as u64 % width) as u32)
        }
        _ => {
            f.pop_to(B);
            None
        }
    };
    let d = f.pop_to_pool();
    let a = &mut f.asm;
    match (sh, imm) {
        (Sh::Shl, Some(c)) => a.lsl_imm(w, d, d, c),
        (Sh::ShrU, Some(c)) => a.lsr_imm(w, d, d, c),
        (Sh::ShrS, Some(c)) => a.asr_imm(w, d, d, c),
        (Sh::Rotr, Some(c)) => {
            a.mov_imm(w, B, c as u64);
            a.rorv(w, d, d, B);
        }
        (Sh::Rotl, Some(c)) => {
            a.mov_imm(w, B, ((width as u32 - c) % width as u32) as u64);
            a.rorv(w, d, d, B);
        }
        (Sh::Shl, None) => a.lslv(w, d, d, B),
        (Sh::ShrU, None) => a.lsrv(w, d, d, B),
        (Sh::ShrS, None) => a.asrv(w, d, d, B),
        (Sh::Rotr, None) => a.rorv(w, d, d, B),
        (Sh::Rotl, None) => {
            a.neg(w, B, B);
            a.rorv(w, d, d, B);
        }
    }
    f.push(ity(w), Loc::Gpr(d));
}

fn cmp(f: &mut Ctx, w: bool, cc: Cond) {
    let rhs = take_rhs(f);
    let home = match f.stack.last().map(|v| v.loc) {
        Some(Loc::Local(i)) => f.home(i),
        _ => None,
    };
    let d = match home {
        Some(h) => {
            f.stack.pop();
            h
        }
        None => f.pop_to_pool(),
    };
    match rhs {
        Rhs::Imm(c) => f.cmp_imm_any(w, d, c),
        Rhs::Reg(r) | Rhs::Owned(r) => f.asm.cmp(w, d, r),
    }
    release_rhs(f, &rhs);
    if home.is_none() {
        f.free_gpr(d);
    }
    f.push_flags(cc);
}

/// The explicit checks x86 gets from #DE: a zero divisor traps, and so does
/// `INT_MIN / -1` for `div_s`. `rem_s` needs no overflow check: `sdiv`
/// answers `INT_MIN` there, and `msub` turns that into the 0 wasm wants.
fn div_op(f: &mut Ctx, w: bool, signed: bool, rem: bool) {
    f.pop_to(B); // divisor
    f.pop_to(A); // dividend
    let p = f.cbz(w, false, B);
    f.trap_patch(trap::DIVIDE_ERROR, p);
    if signed && !rem {
        f.asm.cmn_imm(w, B, 1);
        let skip = f.jcc(Cond::Ne);
        let min = if w { 1u64 << 63 } else { 1u64 << 31 };
        f.asm.mov_imm(w, C, min);
        f.asm.cmp(w, A, C);
        f.trap_if(Cond::Eq, trap::DIVIDE_ERROR);
        f.bind(skip);
    }
    let a = &mut f.asm;
    if rem {
        if signed { a.sdiv(w, C, A, B) } else { a.udiv(w, C, A, B) }
        a.msub(w, A, C, B, A);
    } else if signed {
        a.sdiv(w, A, A, B);
    } else {
        a.udiv(w, A, A, B);
    }
    f.push_from_ty(A, ity(w));
}

fn eqz(f: &mut Ctx, w: bool) {
    if let Some(Loc::Flags(cc)) = f.stack.last().map(|v| v.loc) {
        f.stack.pop();
        f.push_flags(cc.not());
        return;
    }
    f.settle_flags();
    f.pop_to(A);
    f.asm.cmp_imm(w, A, 0);
    f.push_flags(Cond::Eq);
}
