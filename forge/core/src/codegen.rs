//! wasm to x86-64, one function at a time, one pass.
//!
//! The operand stack lives in frame slots, not registers — the simplest thing
//! that can be correct, and the baseline the register cache will later have to
//! beat by a measured amount. Building the cache first would leave no number
//! to compare against.
//!
//! A function that meets an opcode the generator does not emit fails with the
//! opcode's name instead of producing wrong code. That failure is also the
//! progress meter: `--roadmap` counts what `compile()` actually produced, so
//! the report cannot drift from the generator.

use crate::trap;
use crate::vmctx;
use crate::x64::{Asm, Cond, Fw, Patch, Reg, Xmm};
use alloc::vec::Vec;
use wasmparser::{BlockType, Operator, ValType};

/// Where a wasm function's arguments arrive. `rdi` carries the instance
/// context, so the integer arguments start one register later than SysV.
/// Anything past these goes on the stack, exactly as SysV does it: the caller
/// writes them at `[rsp + 8*j]` before the call, so the callee finds them at
/// `[rbp + 16 + 8*j]` once the return address and its own `rbp` are pushed.
const ARG_REGS: [Reg; 5] = [Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];

/// Frame offset of the `j`-th stack argument as the CALLEE sees it.
fn incoming_arg(j: usize) -> i32 {
    16 + 8 * j as i32
}

/// Bytes the caller must reserve for `n` stack arguments, keeping `rsp`
/// 16-aligned across the call.
fn outgoing_bytes(n: usize) -> i32 {
    ((8 * n as i32) + 15) & !15
}

/// Scratch. None holds a live value across an operator boundary.
const A: Reg = Reg::Rax;
const B: Reg = Reg::Rcx;
const C: Reg = Reg::Rdx;
/// A fourth scratch that is NOT an argument register, so a call target can be
/// held while the arguments are loaded on top of `rcx` and `rdx`.
const T: Reg = Reg::R11;

/// The instance context, pinned for the whole function. Callee-saved in SysV,
/// so the frame saves and restores it and generated code may treat it as
/// constant.
const VMCTX: Reg = Reg::R14;

/// Base of linear memory, pinned for the whole function. Worth a register:
/// loads and stores are 10.7 % of beak's instructions, and pinning removes one
/// load from every single one.
///
/// Fuel remaining, as a signed count. Pinned for the same reason as the other
/// two: a resource limit that costs a memory round trip per basic block is
/// what makes Winch pay +87 % for metering where Cranelift pays +24 %, and the
/// difference between those two is the difference between 5x and 10x.
///
/// Signed on purpose — the counter is allowed to go past zero inside a block
/// and is caught at the next check, which is why no check is needed per block.
const FUEL: Reg = Reg::R15;

/// It never has to be reloaded: the instance reserves its address space once
/// and `memory.grow` only makes more of that same range readable, so the base
/// is fixed for the instance's life. An implementation that moved memory on
/// growth would have to reload this after every call — ours does not, and the
/// guard-page reservation is exactly why.
const MEMBASE: Reg = Reg::R13;

/// No frame slots are reserved any more. `r13` and `r14` used to be saved and
/// restored in every function; they are now set ONCE by the entry trampoline
/// and are the same for every function of an instance, so saving them per call
/// was six instructions of pure ceremony. Only the boundary to native code
/// needs them preserved, and that is exactly what the trampoline is.
const RESERVED_SLOTS: u32 = 0;

/// A `call rel32` whose target had no address yet. `at` is the offset of the
/// displacement inside this function's own code; `target` is a wasm function
/// index. The module linker resolves both once every function is placed.
pub struct Reloc {
    pub at: usize,
    pub target: u32,
}

/// Everything the generator needs to know about the module around the
/// function it is translating.
pub struct ModuleCtx<'a> {
    pub types: &'a [(Vec<ValType>, Vec<ValType>)],
    /// Type index per function index, imports first.
    pub func_type_of: &'a [u32],
    pub n_imported: u32,
    /// Canonical signature id per type index.
    pub sig_id: &'a [u32],
    pub globals: &'a [(ValType, bool)],
}

/// The bridge between native code and a module's functions. Native callers
/// have their own idea of `r13`/`r14`, so somebody has to save them, set up
/// the instance's, make the call and put them back — and doing that once at
/// the boundary is cheaper than doing it in every function.
///
/// It is called as
/// `entry(vmctx, target, a, b, c)` and passes the three arguments on.
/// Where a trap lands: name the reason, put the stack back the way the entry
/// trampoline left it, and resume there. Four instructions unwind any depth of
/// wasm frames — and because it needs only `r14`, which generated code never
/// changes, a fault handler can reach it by pointing the interrupted context
/// here with the reason in `rax`.
pub fn emit_trap_routine(asm: &mut Asm) {
    asm.store64(VMCTX, vmctx::TRAP_CODE, Reg::Rax);
    asm.load64(Reg::Rbp, VMCTX, vmctx::TRAP_RBP);
    asm.load64(Reg::Rsp, VMCTX, vmctx::TRAP_RSP);
    asm.jmp_mem(VMCTX, vmctx::TRAP_RESUME);
}

pub fn emit_entry(asm: &mut Asm) {
    asm.push(Reg::Rbp);
    asm.mov_rr64(Reg::Rbp, Reg::Rsp);
    // A fixed frame instead of pushes: three callee-saved registers would
    // leave the stack misaligned for the call, and named offsets read better
    // than counting pushes.
    asm.sub_r64_imm32(Reg::Rsp, 32);
    asm.store64(Reg::Rbp, -8, MEMBASE);
    asm.store64(Reg::Rbp, -16, VMCTX);
    asm.store64(Reg::Rbp, -24, FUEL);

    asm.mov_rr64(VMCTX, Reg::Rdi);

    // Where a trap should come back to, recorded before anything can trap.
    asm.store64(VMCTX, vmctx::TRAP_RBP, Reg::Rbp);
    asm.store64(VMCTX, vmctx::TRAP_RSP, Reg::Rsp);
    let lea_at = asm.lea_rip_blank(Reg::Rax);
    asm.store64(VMCTX, vmctx::TRAP_RESUME, Reg::Rax);
    asm.mov_r32_imm32(Reg::Rax, trap::NONE as i32);
    asm.store64(VMCTX, vmctx::TRAP_CODE, Reg::Rax);

    asm.load64(MEMBASE, VMCTX, vmctx::MEM_BASE);
    asm.load64(FUEL, VMCTX, vmctx::FUEL);

    asm.mov_rr64(Reg::Rax, Reg::Rsi); // the function to enter
    asm.mov_rr64(Reg::Rsi, Reg::Rdx); // and its arguments, shifted down
    asm.mov_rr64(Reg::Rdx, Reg::Rcx);
    asm.mov_rr64(Reg::Rcx, Reg::R8);
    asm.call_reg(Reg::Rax);

    // Both the ordinary return and every trap arrive here.
    let resume = asm.pos();
    asm.patch_i32(lea_at, (resume as i64 - (lea_at as i64 + 4)) as i32);

    // What is left goes back where the runtime can read it.
    asm.store64(VMCTX, vmctx::FUEL, FUEL);
    asm.load64(MEMBASE, Reg::Rbp, -8);
    asm.load64(VMCTX, Reg::Rbp, -16);
    asm.load64(FUEL, Reg::Rbp, -24);
    asm.mov_rr64(Reg::Rsp, Reg::Rbp);
    asm.pop(Reg::Rbp);
    asm.ret();
}

pub struct CompiledFunc {
    pub code: Vec<u8>,
    pub relocs: Vec<Reloc>,
    /// Offsets of `jmp rel32`s aimed at the module's trap routine.
    pub trap_relocs: Vec<usize>,
    /// Bytes of frame below `rbp`, already 16-aligned.
    pub frame: u32,
    /// Deepest the operand stack got.
    pub max_stack: u32,
}

pub enum Outcome {
    Done(CompiledFunc),
    /// The opcode that stopped it. Naming it is the whole point.
    Unsupported(&'static str),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Kind {
    /// The function body itself. Branching to it leaves the function.
    Func,
    Block,
    Loop,
    If,
}

struct Ctrl {
    kind: Kind,
    /// Operand stack height when the frame was entered.
    height: u32,
    /// Values a branch to this label carries. A loop's label is its head, and
    /// without multi-value a loop has no parameters — so zero.
    branch_arity: u32,
    /// Values left on the stack when control falls out of the frame, and of
    /// what type — the operand stack has to be rebuilt exactly at every end.
    out_arity: u32,
    out_ty: ValType,
    /// Loop only: code position of the head, the target of a back edge.
    start: usize,
    /// Forward branches waiting for this frame's end.
    ends: Vec<Patch>,
    /// `If` only: the edge taken when the condition is false.
    else_patch: Option<Patch>,
    /// Was the frame entered from unreachable code? Then it emits nothing.
    dead: bool,
}

/// Does this value need REX.W — that is, is it a 64-bit INTEGER? Floats never
/// answer yes here; their width is carried by `Fw`.
fn w64(t: ValType) -> bool {
    matches!(t, ValType::I64)
}

/// Does the value occupy all eight bytes of its slot? True for i64 AND f64,
/// which is a different question from `w64` and used in different places —
/// conflating the two is how an f64 ends up half-stored.
fn wide(t: ValType) -> bool {
    matches!(t, ValType::I64 | ValType::F64)
}

fn is_float(t: ValType) -> bool {
    matches!(t, ValType::F32 | ValType::F64)
}

fn fw_of(t: ValType) -> Fw {
    if matches!(t, ValType::F64) { Fw::Double } else { Fw::Single }
}

/// Scratch float registers. `X0` doubles as the float return register and the
/// first float argument register, which is harmless: arguments are loaded
/// immediately before a call and the result is taken immediately after.
const FA: Xmm = Xmm::X0;
const FB: Xmm = Xmm::X1;
const FC: Xmm = Xmm::X2;

/// Float arguments have their OWN sequence of registers in SysV, counted
/// separately from the integer ones. A signature of (i32, f64, i32) puts the
/// integers in the first two integer slots and the double in `xmm0` — not in
/// "the second argument register".
const FARG_REGS: [Xmm; 8] = [
    Xmm::X0, Xmm::X1, Xmm::X2, Xmm::X3, Xmm::X4, Xmm::X5, Xmm::X6, Xmm::X7,
];

/// Where one parameter travels.
#[derive(Copy, Clone)]
enum Place {
    Int(usize),
    Flt(usize),
    /// Index of the eight-byte stack slot, counted from the first one.
    Stack(usize),
}

/// Assign every parameter its place. Both sides of a call run this, so caller
/// and callee cannot disagree about where an argument is.
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

/// Where an operand-stack value actually is. `Slot` means it has been written
/// to its own frame slot, which is the canonical place and the only one a
/// branch target or a callee may assume.
#[derive(Copy, Clone, PartialEq)]
enum Loc {
    Gpr(Reg),
    Xmm(Xmm),
    Slot,
    /// A constant that has not been put anywhere yet.
    Imm(i64),
    /// Still sitting in its local's frame slot. `local.get` copies a value in
    /// wasm, so nothing needs to move until somebody actually wants it — and
    /// `LocalGet` alone is 23,4 % of all instructions, `I32Const` another 17 %.
    /// Between them that is two fifths of every push, and the cheapest way to
    /// serve a push is not to emit anything at all.
    ///
    /// The catch is that a local can be WRITTEN between the get and the use,
    /// and wasm copied the value at the get. `local.set`/`local.tee` therefore
    /// have to settle any pending reference to the local they are about to
    /// overwrite.
    Local(u32),
}

#[derive(Copy, Clone)]
struct Val {
    ty: ValType,
    loc: Loc,
}

/// The registers the operand stack may live in — deliberately DISJOINT from
/// the operators' scratch (`rax`, `rcx`, `rdx`, `r11` and `xmm0..2`).
///
/// That separation is what makes the cache cheap to introduce: no operator
/// has to learn about allocation, because nothing it writes can ever hold a
/// stack value. The price is a register move in and out instead of leaving a
/// value where it was produced — and on this hardware a register-to-register
/// move is usually eliminated in the rename stage, while a store followed by
/// a load is not.
///
/// All of them are caller-saved in SysV, so spilling before a call is enough;
/// `rbx` and `r12` stay out rather than being saved in every prologue.
/// `rsi`/`rdi` are in the pool even though `rep movsb` needs them, because
/// bulk memory spills everything first anyway.
const GPRS: [Reg; 5] = [Reg::Rsi, Reg::Rdi, Reg::R8, Reg::R9, Reg::R10];

const XMMS: [Xmm; 5] = [Xmm::X3, Xmm::X4, Xmm::X5, Xmm::X6, Xmm::X7];

struct Ctx<'a> {
    asm: Asm,
    m: &'a ModuleCtx<'a>,
    /// Which registers are spoken for — by a stack value, or by an operator
    /// that has taken one out and not given it back yet.
    gpr_used: [bool; 16],
    xmm_used: [bool; 16],
    /// Type of every local, parameters first. A local's slot holds a value of
    /// exactly this width — nothing reads the bytes above it.
    local_types: Vec<ValType>,
    relocs: Vec<Reloc>,
    /// Branches out of the function, grouped by what went wrong. Each group
    /// gets its own two-instruction stub after the epilogue, so the common
    /// path falls straight through and the reason survives to the runtime.
    traps: Vec<(u32, Vec<Patch>)>,
    /// Offsets of the `jmp rel32`s in those stubs; the module linker points
    /// them at the trap routine.
    trap_relocs: Vec<usize>,
    n_locals: u32,
    /// The operand stack. Its LENGTH is the depth — one source of truth, so a
    /// push that forgets its type cannot happen.
    stack: Vec<Val>,
    max_stack: u32,
    frame_patch: usize,
    ctrl: Vec<Ctrl>,
    /// Instructions counted since the last time the fuel register was
    /// updated. Charged in one go at the next control-flow edge, so metering
    /// costs one `sub` per basic block — measured at 5,8 wasm instructions —
    /// instead of one per instruction.
    fuel_pending: i64,
    /// False after a branch, until an `else` or an `end` brings control back.
    /// Unreachable code is not emitted: the validator types it
    /// polymorphically, so tracking a stack through it would be fiction.
    reachable: bool,
}

impl<'a> Ctx<'a> {
    /// Frame slot for local `i`, relative to `rbp`.
    fn local(&self, i: u32) -> i32 {
        -8 * (i as i32 + RESERVED_SLOTS as i32 + 1)
    }

    /// Frame slot for operand-stack entry `d`. Every value has one whether it
    /// is currently in a register or not — a spill needs somewhere to go, and
    /// the place must not depend on when the spill happens.
    fn slot(&self, d: u32) -> i32 {
        -8 * (RESERVED_SLOTS as i32 + self.n_locals as i32 + d as i32 + 1)
    }

    fn depth(&self) -> u32 {
        self.stack.len() as u32
    }

    fn peek_ty(&self) -> Option<ValType> {
        self.stack.last().map(|v| v.ty)
    }

    /// A constant, remembered rather than emitted.
    fn push_imm(&mut self, v: i64, t: ValType) {
        self.stack.push(Val { ty: t, loc: Loc::Imm(v) });
        self.note_depth();
    }

    /// A copy of a local, remembered rather than loaded.
    fn push_local(&mut self, idx: u32, t: ValType) {
        self.stack.push(Val { ty: t, loc: Loc::Local(idx) });
        self.note_depth();
    }

    /// Settle every pending reference to local `idx` — it is about to change,
    /// and wasm took its copy earlier.
    fn settle_local(&mut self, idx: u32) {
        for i in 0..self.stack.len() {
            if self.stack[i].loc == Loc::Local(idx) {
                self.spill(i);
            }
        }
    }

    /// Note a value that is already in its slot — what a block end leaves
    /// behind, since everything is spilled before control can join.
    fn push_slot(&mut self, t: ValType) {
        self.stack.push(Val { ty: t, loc: Loc::Slot });
        self.note_depth();
    }

    // --- registers ---

    fn alloc_gpr(&mut self) -> Reg {
        for r in GPRS {
            if !self.gpr_used[r as usize] {
                self.gpr_used[r as usize] = true;
                return r;
            }
        }
        // Nothing free: the value that has waited longest goes to memory.
        // Spilling the deepest keeps the ones an operator is about to want.
        self.spill_deepest_gpr();
        for r in GPRS {
            if !self.gpr_used[r as usize] {
                self.gpr_used[r as usize] = true;
                return r;
            }
        }
        Reg::Rax
    }

    fn alloc_xmm(&mut self) -> Xmm {
        for x in XMMS {
            if !self.xmm_used[x as usize] {
                self.xmm_used[x as usize] = true;
                return x;
            }
        }
        self.spill_deepest_xmm();
        for x in XMMS {
            if !self.xmm_used[x as usize] {
                self.xmm_used[x as usize] = true;
                return x;
            }
        }
        Xmm::X0
    }

    fn free_gpr(&mut self, r: Reg) {
        self.gpr_used[r as usize] = false;
    }
    fn free_xmm(&mut self, x: Xmm) {
        self.xmm_used[x as usize] = false;
    }

    /// Write the value at `i` to its own slot and let go of its register.
    ///
    /// Materialising through `r11`: it is neither a cache register nor
    /// anything an operator holds while a spill can happen — spills come from
    /// `alloc_gpr` and from `spill_all`, and both run between operators.
    fn spill(&mut self, i: usize) {
        let v = self.stack[i];
        let off = self.slot(i as u32);
        match v.loc {
            Loc::Imm(c) => {
                self.asm.mov_r64_imm64(T, c);
                if wide(v.ty) {
                    self.asm.store64(Reg::Rbp, off, T);
                } else {
                    self.asm.store32(Reg::Rbp, off, T);
                }
            }
            Loc::Local(idx) => {
                let src = self.local(idx);
                if wide(v.ty) {
                    self.asm.load64(T, Reg::Rbp, src);
                    self.asm.store64(Reg::Rbp, off, T);
                } else {
                    self.asm.load32(T, Reg::Rbp, src);
                    self.asm.store32(Reg::Rbp, off, T);
                }
            }
            Loc::Gpr(r) => {
                if wide(v.ty) {
                    self.asm.store64(Reg::Rbp, off, r);
                } else {
                    self.asm.store32(Reg::Rbp, off, r);
                }
                self.free_gpr(r);
            }
            Loc::Xmm(x) => {
                self.asm.fstore_slot(fw_of(v.ty), Reg::Rbp, off, x);
                self.free_xmm(x);
            }
            Loc::Slot => return,
        }
        self.stack[i].loc = Loc::Slot;
    }

    fn spill_deepest_gpr(&mut self) {
        for i in 0..self.stack.len() {
            if matches!(self.stack[i].loc, Loc::Gpr(_)) {
                self.spill(i);
                return;
            }
        }
    }

    fn spill_deepest_xmm(&mut self) {
        for i in 0..self.stack.len() {
            if matches!(self.stack[i].loc, Loc::Xmm(_)) {
                self.spill(i);
                return;
            }
        }
    }

    /// Put the whole operand stack where everyone else expects to find it.
    /// Mandatory before a call (the registers do not survive it) and at every
    /// point where control can join — a branch target cannot know which
    /// register a value happened to be in on the way there.
    fn spill_all(&mut self) {
        for i in 0..self.stack.len() {
            self.spill(i);
        }
    }

    /// Make `r` available: whatever stack value holds it goes to its slot.
    fn evict_gpr(&mut self, r: Reg) {
        for i in 0..self.stack.len() {
            if self.stack[i].loc == Loc::Gpr(r) {
                self.spill(i);
                return;
            }
        }
    }

    fn evict_xmm(&mut self, x: Xmm) {
        for i in 0..self.stack.len() {
            if self.stack[i].loc == Loc::Xmm(x) {
                self.spill(i);
                return;
            }
        }
    }

    /// Drop the stack back to `h` entries, releasing their registers.
    fn truncate_to(&mut self, h: u32) {
        while self.stack.len() > h as usize {
            match self.stack.pop().map(|v| v.loc) {
                Some(Loc::Gpr(r)) => self.free_gpr(r),
                Some(Loc::Xmm(x)) => self.free_xmm(x),
                _ => {}
            }
        }
    }

    // --- the operand stack ---

    fn note_depth(&mut self) {
        if self.depth() > self.max_stack {
            self.max_stack = self.depth();
        }
    }

    /// The float equivalents of `push_reg` / `pop_to_pool`.
    fn fpush_reg(&mut self, x: Xmm, t: ValType) {
        self.stack.push(Val { ty: t, loc: Loc::Xmm(x) });
        self.note_depth();
    }

    fn fpop_to_pool(&mut self) -> Xmm {
        if let Some(v) = self.stack.last().copied() {
            if let Loc::Xmm(x) = v.loc {
                self.stack.pop();
                return x;
            }
        }
        let d = self.alloc_xmm();
        self.fpop_to(d);
        d
    }

    /// The value is already in a cache register and stays there. What the hot
    /// operators use, so that an arithmetic result costs no move at all.
    fn push_reg(&mut self, r: Reg, t: ValType) {
        self.stack.push(Val { ty: t, loc: Loc::Gpr(r) });
        self.note_depth();
    }

    /// Take the top into a CACHE register and keep it there — in place if it
    /// already is one. The caller may write it and hand it straight back with
    /// `push_reg`, which is how the last move disappears from the hot path.
    fn pop_to_pool(&mut self) -> Reg {
        if let Some(v) = self.stack.last().copied() {
            if let Loc::Gpr(r) = v.loc {
                self.stack.pop();
                return r; // still marked used: it belongs to the caller now
            }
        }
        let d = self.alloc_gpr();
        self.pop_to(d);
        d
    }

    /// The operator produced the value in its own scratch register; give it a
    /// place the stack can keep it. When every cache register is taken the
    /// deepest one goes to memory, which is the case this whole arrangement is
    /// trying to make rare.
    fn push_from_ty(&mut self, r: Reg, t: ValType) {
        let d = self.alloc_gpr();
        if d != r {
            if wide(t) {
                self.asm.mov_rr64(d, r);
            } else {
                self.asm.mov_rr32(d, r);
            }
        }
        self.stack.push(Val { ty: t, loc: Loc::Gpr(d) });
        self.note_depth();
    }

    fn push_from(&mut self, r: Reg) {
        self.push_from_ty(r, ValType::I32);
    }

    fn fpush_from(&mut self, x: Xmm, t: ValType) {
        let d = self.alloc_xmm();
        if d != x {
            self.asm.fmov(d, x);
        }
        self.stack.push(Val { ty: t, loc: Loc::Xmm(d) });
        self.note_depth();
    }

    /// Take the top value into `r`, whatever it costs: nothing if it is
    /// already there, a register move if it is elsewhere, a load if it was
    /// spilled. The caller owns `r` afterwards and must give it back.
    /// Take the top value into `r`. `r` is operator scratch and never holds a
    /// stack value, so nothing has to be evicted first.
    fn pop_to(&mut self, r: Reg) -> Option<ValType> {
        let v = self.stack.pop()?;
        match v.loc {
            Loc::Gpr(x) => {
                if wide(v.ty) {
                    self.asm.mov_rr64(r, x);
                } else {
                    self.asm.mov_rr32(r, x);
                }
                self.free_gpr(x);
            }
            Loc::Xmm(x) => {
                // Only the bits are wanted; this is how a float reaches a
                // general register for a slot move or a reinterpret.
                self.asm.xmm_to_gpr(wide(v.ty), r, x);
                self.free_xmm(x);
            }
            Loc::Slot => {
                let off = self.slot(self.depth());
                if wide(v.ty) {
                    self.asm.load64(r, Reg::Rbp, off);
                } else {
                    self.asm.load32(r, Reg::Rbp, off);
                }
            }
            Loc::Imm(c) => {
                if wide(v.ty) {
                    self.asm.mov_r64_imm64(r, c);
                } else {
                    self.asm.mov_r32_imm32(r, c as i32);
                }
            }
            Loc::Local(idx) => {
                let off = self.local(idx);
                if wide(v.ty) {
                    self.asm.load64(r, Reg::Rbp, off);
                } else {
                    self.asm.load32(r, Reg::Rbp, off);
                }
            }
        }
        Some(v.ty)
    }

    /// The same for the float register file.
    fn fpop_to(&mut self, x: Xmm) -> Option<ValType> {
        let v = self.stack.pop()?;
        match v.loc {
            Loc::Xmm(y) => {
                self.asm.fmov(x, y);
                self.free_xmm(y);
            }
            Loc::Gpr(r) => {
                self.asm.gpr_to_xmm(wide(v.ty), x, r);
                self.free_gpr(r);
            }
            Loc::Slot => {
                let off = self.slot(self.depth());
                self.asm.fload_slot(fw_of(v.ty), x, Reg::Rbp, off);
            }
            Loc::Local(idx) => {
                let off = self.local(idx);
                self.asm.fload_slot(fw_of(v.ty), x, Reg::Rbp, off);
            }
            Loc::Imm(c) => {
                self.asm.mov_r64_imm64(T, c);
                self.asm.gpr_to_xmm(wide(v.ty), x, T);
            }
        }
        Some(v.ty)
    }

    /// Read the top into `r` without consuming it. The value keeps its own
    /// place, so `r` is a copy and the caller must free it.
    /// Read the top into `r` without consuming it — the value keeps its place.
    fn peek_to(&mut self, r: Reg) {
        let Some(v) = self.stack.last().copied() else { return };
        match v.loc {
            Loc::Gpr(x) => {
                if wide(v.ty) {
                    self.asm.mov_rr64(r, x);
                } else {
                    self.asm.mov_rr32(r, x);
                }
            }
            Loc::Xmm(x) => self.asm.xmm_to_gpr(wide(v.ty), r, x),
            Loc::Slot => {
                let off = self.slot(self.depth() - 1);
                if wide(v.ty) {
                    self.asm.load64(r, Reg::Rbp, off);
                } else {
                    self.asm.load32(r, Reg::Rbp, off);
                }
            }
            Loc::Imm(c) => {
                if wide(v.ty) {
                    self.asm.mov_r64_imm64(r, c);
                } else {
                    self.asm.mov_r32_imm32(r, c as i32);
                }
            }
            Loc::Local(idx) => {
                let off = self.local(idx);
                if wide(v.ty) {
                    self.asm.load64(r, Reg::Rbp, off);
                } else {
                    self.asm.load32(r, Reg::Rbp, off);
                }
            }
        }
    }

    /// Load the globals array base into `r`.
    fn globals_base(&mut self, r: Reg) {
        self.asm.load64(r, VMCTX, vmctx::GLOBALS);
    }

    /// Charge everything counted so far. Must come BEFORE anything that sets
    /// flags for a branch — `sub` writes the flags too.
    fn fuel_flush(&mut self) {
        if self.fuel_pending == 0 {
            return;
        }
        let n = self.fuel_pending.clamp(0, i32::MAX as i64) as i32;
        self.asm.sub_r64_imm32(FUEL, n);
        self.fuel_pending = 0;
    }

    /// Is there any fuel left? Only needed where execution could otherwise run
    /// forever — a loop's back edge and a function's entry. Straight-line code
    /// cannot loop, so it needs no check, and the counter simply goes negative
    /// until the next one.
    fn fuel_check(&mut self) {
        self.asm.test(true, FUEL, FUEL);
        // `jle`, not `js`: a budget of exactly zero is spent too, and a check
        // that only fired on negative would let one more block through.
        let p = self.asm.jcc(Cond::Le);
        self.trap_patch(trap::OUT_OF_FUEL, p);
    }

    fn trap_patch(&mut self, code: u32, p: Patch) {
        if let Some(e) = self.traps.iter_mut().find(|(c, _)| *c == code) {
            e.1.push(p);
        } else {
            self.traps.push((code, alloc::vec![p]));
        }
    }

    fn trap_if(&mut self, cc: Cond, code: u32) {
        let p = self.asm.jcc(cc);
        self.trap_patch(code, p);
    }

    fn trap_now(&mut self, code: u32) {
        let p = self.asm.jmp();
        self.trap_patch(code, p);
    }
}

/// Every numeric type wasm has. What is left out is the reference types, and
/// `features()` already refuses those before the generator ever sees them.
fn int_ty(t: ValType) -> bool {
    matches!(
        t,
        ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64
    )
}

/// Values a block type leaves behind, and of what type. Without multi-value
/// there are only two shapes, and a type index would be one — so it is refused
/// rather than mishandled.
fn block_out(ty: BlockType) -> Result<(u32, ValType), &'static str> {
    match ty {
        BlockType::Empty => Ok((0, ValType::I32)),
        BlockType::Type(t) if int_ty(t) => Ok((1, t)),
        BlockType::Type(_) => Err("blocktype-value"),
        BlockType::FuncType(_) => Err("blocktype-multivalue"),
    }
}

pub fn compile_func(
    params: &[ValType],
    results: &[ValType],
    local_decls: &[(u32, ValType)],
    ops: &[Operator<'_>],
    m: &ModuleCtx<'_>,
) -> Outcome {
    if !params.iter().copied().all(int_ty) {
        return Outcome::Unsupported("param-type");
    }
    if results.len() > 1 || !results.iter().copied().all(int_ty) {
        return Outcome::Unsupported("result-type");
    }
    if !local_decls.iter().all(|(_, t)| int_ty(*t)) {
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

    let mut f = Ctx {
        asm: Asm::new(),
        m,
        local_types,
        relocs: Vec::new(),
        traps: Vec::new(),
        trap_relocs: Vec::new(),
        n_locals,
        fuel_pending: 0,
        stack: Vec::new(),
        gpr_used: [false; 16],
        xmm_used: [false; 16],
        max_stack: 0,
        frame_patch: 0,
        ctrl: Vec::new(),
        reachable: true,
    };

    // The function body is a frame like any other, so `br` to the outermost
    // depth needs no special case — it is a branch out of the function.
    f.ctrl.push(Ctrl {
        kind: Kind::Func,
        height: 0,
        branch_arity: out_arity,
        out_arity,
        out_ty: results.first().copied().unwrap_or(ValType::I32),
        start: 0,
        ends: Vec::new(),
        else_patch: None,
        dead: false,
    });

    // Prologue. The frame size is not known yet, so the immediate is written
    // as zero and patched at the end.
    f.asm.push(Reg::Rbp);
    f.asm.mov_rr64(Reg::Rbp, Reg::Rsp);
    f.asm.sub_r64_imm32(Reg::Rsp, 0);
    f.frame_patch = f.asm.pos() - 4;
    let (places, _) = arg_places(params);
    for (i, t) in params.iter().enumerate() {
        let off = f.local(i as u32);
        match places[i] {
            Place::Int(k) => {
                if wide(*t) {
                    f.asm.store64(Reg::Rbp, off, ARG_REGS[k]);
                } else {
                    f.asm.store32(Reg::Rbp, off, ARG_REGS[k]);
                }
            }
            Place::Flt(k) => f.asm.fstore_slot(fw_of(*t), Reg::Rbp, off, FARG_REGS[k]),
            Place::Stack(j) => {
                // A stack argument always fills a whole eight-byte slot, so
                // the raw bits can be moved through a general register no
                // matter what the type is.
                let src = incoming_arg(j);
                if wide(*t) {
                    f.asm.load64(A, Reg::Rbp, src);
                    f.asm.store64(Reg::Rbp, off, A);
                } else {
                    f.asm.load32(A, Reg::Rbp, src);
                    f.asm.store32(Reg::Rbp, off, A);
                }
            }
        }
    }
    // Declared locals are zero in wasm, and nothing else may be assumed. The
    // full eight bytes are cleared regardless of width — cheaper than deciding
    // per local, and it leaves no stale upper half behind.
    if n_declared > 0 {
        f.asm.mov_r32_imm32(A, 0);
        for i in params.len() as u32..n_locals {
            let off = f.local(i);
            f.asm.store64(Reg::Rbp, off, A);
        }
    }

    // Entry check: with one at every loop head and one here, no path can run
    // unbounded without meeting one.
    f.fuel_check();

    for op in ops {
        if let Err(name) = emit(&mut f, op) {
            return Outcome::Unsupported(name);
        }
    }
    if !f.ctrl.is_empty() {
        return Outcome::Unsupported("unclosed-frame");
    }

    f.fuel_flush();

    // Epilogue. Every path arrives with the result, if any, in slot 0.
    if let Some(rt) = results.first().copied() {
        let off = f.slot(0);
        if is_float(rt) {
            f.asm.fload_slot(fw_of(rt), FA, Reg::Rbp, off);
        } else if wide(rt) {
            f.asm.load64(A, Reg::Rbp, off);
        } else {
            f.asm.load32(A, Reg::Rbp, off);
        }
    }
    f.asm.mov_rr64(Reg::Rsp, Reg::Rbp);
    f.asm.pop(Reg::Rbp);
    f.asm.ret();

    // One stub per reason, after the return path so the common path falls
    // straight through. Two instructions: name the reason, leave.
    let traps = core::mem::take(&mut f.traps);
    for (code, patches) in traps {
        for p in patches {
            f.asm.bind(p);
        }
        f.asm.mov_r32_imm32(A, code as i32);
        let at = f.asm.jmp_rel32_blank();
        f.trap_relocs.push(at);
    }

    let slots = RESERVED_SLOTS + n_locals + f.max_stack.max(1);
    let frame = (8 * slots + 15) & !15;
    let patch = f.frame_patch;
    f.asm.code[patch..patch + 4].copy_from_slice(&(frame as i32).to_le_bytes());

    let max_stack = f.max_stack;
    let relocs = core::mem::take(&mut f.relocs);
    let trap_relocs = core::mem::take(&mut f.trap_relocs);
    match f.asm.finish() {
        Some(code) => Outcome::Done(CompiledFunc {
            code,
            relocs,
            trap_relocs,
            frame,
            max_stack,
        }),
        None => Outcome::Unsupported("open-branch"),
    }
}

fn emit(f: &mut Ctx, op: &Operator<'_>) -> Result<(), &'static str> {
    use Operator::*;

    // One unit per operator — but NOT for the purely structural ones.
    //
    // This lands within 1,9 % of the interpreter's own count on beak's warm
    // layout (709 681 859 against 723 152 293). Charging bulk memory by the
    // BYTE was tried and is wrong — it overshoots by 20-30 %, so the
    // interpreter bills a copy roughly flat. What the last two per cent are
    // is still open; it is close enough that the kernel's existing budgets
    // keep their meaning, which was the point. A
    // `block`, a `loop`, an `else` or an `end` is a bracket in the binary
    // format, not something that runs; the interpreter charges nothing for
    // them either. Matching its rule matters beyond tidiness: the kernel's
    // budgets (10 G for `run`, PYTHON_FUEL) were all calibrated against the
    // interpreter, and they have to keep meaning the same thing.
    if f.reachable && !matches!(op, Block { .. } | Loop { .. } | Else | End | Nop) {
        f.fuel_pending += 1;
    }

    // Frames are tracked even where no code is emitted, because the nesting
    // is what tells us when control becomes reachable again.
    match op {
        Block { blockty } => {
            f.fuel_flush();
            let (out, out_ty) = block_out(*blockty)?;
            let dead = !f.reachable;
            f.ctrl.push(Ctrl {
                kind: Kind::Block,
                height: f.depth(),
                branch_arity: out,
                out_arity: out,
                out_ty,
                start: 0,
                ends: Vec::new(),
                else_patch: None,
                dead,
            });
            return Ok(());
        }
        Loop { blockty } => {
            f.fuel_flush();
            let (out, out_ty) = block_out(*blockty)?;
            let dead = !f.reachable;
            // The head is a branch target: the back edge arrives with
            // everything in its slot, so the way in must match.
            if !dead {
                f.spill_all();
            }
            // Mark the head BEFORE the check, not after. A loop is the only
            // place execution can go round for ever, so the check has to sit
            // ON the back edge — recorded after it, the edge jumps straight
            // past and the loop is checked exactly once, on the way in.
            let head = f.asm.pos();
            if !dead {
                f.fuel_check();
            }
            f.ctrl.push(Ctrl {
                kind: Kind::Loop,
                height: f.depth(),
                // A branch to a loop goes to its head and carries the loop's
                // parameters. Without multi-value there are none.
                branch_arity: 0,
                out_arity: out,
                out_ty,
                start: head,
                ends: Vec::new(),
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
                f.pop_to(A);
                // Both arms start from this point, and the else-arm is
                // reached long after the then-arm has had its way with the
                // registers. Settling here is what keeps the two agreeing.
                f.spill_all();
                f.asm.test32(A, A);
                Some(f.asm.jcc(Cond::E))
            };
            f.ctrl.push(Ctrl {
                kind: Kind::If,
                height: f.depth(),
                branch_arity: out,
                out_arity: out,
                out_ty,
                start: 0,
                ends: Vec::new(),
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
            // The then-arm, if it can fall out, skips the else-arm.
            if f.reachable {
                f.spill_all();
                let p = f.asm.jmp();
                f.ctrl[top].ends.push(p);
            }
            let Some(ep) = f.ctrl[top].else_patch.take() else {
                return Err("double-else");
            };
            f.asm.bind(ep);
            f.reachable = true;
            let h = f.ctrl[top].height;
            f.truncate_to(h);
            return Ok(());
        }
        End => {
            f.fuel_flush();
            // Control joins here — the fall-through and every branch that
            // aimed at this end must leave the operand stack looking the same.
            if f.reachable {
                f.spill_all();
            }
            let Some(mut fr) = f.ctrl.pop() else {
                return Err("end-without-frame");
            };
            if !fr.dead {
                // An `if` with no `else` still needs its false edge to land
                // somewhere, and that somewhere is here.
                if let Some(ep) = fr.else_patch.take() {
                    f.asm.bind(ep);
                    f.reachable = true;
                }
                let branched = !fr.ends.is_empty();
                while let Some(p) = fr.ends.pop() {
                    f.asm.bind(p);
                }
                f.reachable |= branched;
            } else {
                f.reachable = false;
            }
            f.truncate_to(fr.height);
            if fr.out_arity == 1 {
                f.push_slot(fr.out_ty);
            }
            return Ok(());
        }
        Br { relative_depth } => {
            f.fuel_flush();
            if f.reachable {
                f.spill_all();
                branch_to(f, *relative_depth)?;
                f.reachable = false;
            }
            return Ok(());
        }
        Return => {
            f.fuel_flush();
            if f.reachable {
                f.spill_all();
                let d = f.ctrl.len() as u32 - 1;
                branch_to(f, d)?;
                f.reachable = false;
            }
            return Ok(());
        }
        BrTable { targets } => {
            f.fuel_flush();
            if f.reachable {
                f.spill_all();
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
            // Dropping has to hand the register back, or the allocator would
            // lose one per discarded value.
            let d = f.depth();
            f.truncate_to(d.saturating_sub(1));
        }

        LocalGet { local_index } => {
            let t = *f
                .local_types
                .get(*local_index as usize)
                .ok_or("local-index")?;
            f.push_local(*local_index, t);
        }
        LocalSet { local_index } => {
            f.settle_local(*local_index);
            // Only the bits move, so a general register does for floats too.
            let t = f.pop_to(A).ok_or("stack-empty")?;
            let off = f.local(*local_index);
            if wide(t) {
                f.asm.store64(Reg::Rbp, off, A);
            } else {
                f.asm.store32(Reg::Rbp, off, A);
            }
        }
        LocalTee { local_index } => {
            f.settle_local(*local_index);
            let t = f.peek_ty().ok_or("stack-empty")?;
            f.peek_to(A);
            let off = f.local(*local_index);
            if wide(t) {
                f.asm.store64(Reg::Rbp, off, A);
            } else {
                f.asm.store32(Reg::Rbp, off, A);
            }
        }

        GlobalGet { global_index } => {
            match f.m.globals.get(*global_index as usize) {
                Some((t, _)) if int_ty(*t) => {}
                Some(_) => return Err("global-type"),
                None => return Err("global-index"),
            }
            let t = f.m.globals[*global_index as usize].0;
            f.globals_base(B);
            let off = vmctx::GLOBAL_STRIDE * *global_index as i32;
            if is_float(t) {
                f.asm.fload_slot(fw_of(t), FA, B, off);
                f.fpush_from(FA, t);
            } else {
                if wide(t) {
                    f.asm.load64(A, B, off);
                } else {
                    f.asm.load32(A, B, off);
                }
                f.push_from_ty(A, t);
            }
        }
        GlobalSet { global_index } => {
            match f.m.globals.get(*global_index as usize) {
                Some((t, true)) if int_ty(*t) => {}
                Some((t, false)) if int_ty(*t) => return Err("global-immutable"),
                Some(_) => return Err("global-type"),
                None => return Err("global-index"),
            }
            let t = f.pop_to(A).ok_or("stack-empty")?;
            f.globals_base(B);
            let off = vmctx::GLOBAL_STRIDE * *global_index as i32;
            if wide(t) {
                f.asm.store64(B, off, A);
            } else {
                f.asm.store32(B, off, A);
            }
        }

        I32Const { value } => f.push_imm(*value as i64, ValType::I32),
        I64Const { value } => f.push_imm(*value, ValType::I64),

        I32Add => bin(f, false, ADD),
        I32Sub => bin(f, false, SUB),
        I32Mul => mul(f, false),
        I32And => bin(f, false, AND),
        I32Or => bin(f, false, OR),
        I32Xor => bin(f, false, XOR),
        I64Add => bin(f, true, ADD),
        I64Sub => bin(f, true, SUB),
        I64Mul => mul(f, true),
        I64And => bin(f, true, AND),
        I64Or => bin(f, true, OR),
        I64Xor => bin(f, true, XOR),

        // The shift count lands in `rcx` by construction: `bin` pops the
        // right-hand operand there first, and `cl` is where x86 wants it.
        // wasm masks the count to the operand's bit width, and so does x86.
        I32Shl => shift(f, false, 4),
        I32ShrU => shift(f, false, 5),
        I32ShrS => shift(f, false, 7),
        I32Rotl => shift(f, false, 0),
        I32Rotr => shift(f, false, 1),
        I64Shl => shift(f, true, 4),
        I64ShrU => shift(f, true, 5),
        I64ShrS => shift(f, true, 7),
        I64Rotl => shift(f, true, 0),
        I64Rotr => shift(f, true, 1),

        // Comparisons take operands of their own width and always leave an
        // i32 behind, whatever went in.
        I32Eq => cmp(f, false, Cond::E),
        I32Ne => cmp(f, false, Cond::Ne),
        I32LtS => cmp(f, false, Cond::L),
        I32LtU => cmp(f, false, Cond::B),
        I32GtS => cmp(f, false, Cond::G),
        I32GtU => cmp(f, false, Cond::A),
        I32LeS => cmp(f, false, Cond::Le),
        I32LeU => cmp(f, false, Cond::Be),
        I32GeS => cmp(f, false, Cond::Ge),
        I32GeU => cmp(f, false, Cond::Ae),
        I64Eq => cmp(f, true, Cond::E),
        I64Ne => cmp(f, true, Cond::Ne),
        I64LtS => cmp(f, true, Cond::L),
        I64LtU => cmp(f, true, Cond::B),
        I64GtS => cmp(f, true, Cond::G),
        I64GtU => cmp(f, true, Cond::A),
        I64LeS => cmp(f, true, Cond::Le),
        I64LeU => cmp(f, true, Cond::Be),
        I64GeS => cmp(f, true, Cond::Ge),
        I64GeU => cmp(f, true, Cond::Ae),

        // --- division ---
        //
        // wasm traps on a zero divisor, and `div_s` traps once more on
        // `INT_MIN / -1`. x86 raises #DE for BOTH of those and for nothing
        // else once the high half is set up — after `cdq`/`cqo` the dividend
        // is exactly the operand, so the only quotient that fails to fit is
        // `INT_MIN / -1`. So the hardware's condition IS wasm's condition, and
        // no compare is emitted.
        //
        // That leaves one operator out of step: `rem_s` must NOT trap on
        // `INT_MIN % -1` — the answer is 0. A divisor of -1 gives a remainder
        // of zero for EVERY dividend, so the shortcut is a plain special case
        // rather than a check for the pair.
        //
        // The price: the trap handler has to claim #DE from generated code,
        // exactly as it has to claim #PF from a guard page. Until it exists,
        // both are equally fatal.
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

        // `bsr`/`bsf` leave the destination undefined for a zero input and
        // say so in ZF. The sentinel is arranged so the SAME final `xor`
        // produces wasm's answer, which keeps the whole thing branch-free.
        I32Clz => clz(f, false),
        I64Clz => clz(f, true),
        I32Ctz => ctz(f, false),
        I64Ctz => ctz(f, true),
        I32Popcnt => {
            f.pop_to(A);
            f.asm.popcnt(false, A, A);
            f.push_from_ty(A, ValType::I32);
        }

        // Width conversions. `pop_to` already read the value at its own
        // width, so wrapping needs no instruction at all — the narrower store
        // does it, and the zero-extending direction is already done by the
        // 32-bit load.
        I32WrapI64 => {
            f.pop_to(A);
            f.push_from_ty(A, ValType::I32);
        }
        I64ExtendI32U => {
            f.pop_to(A);
            f.push_from_ty(A, ValType::I64);
        }
        I64ExtendI32S => {
            f.pop_to(A);
            f.asm.movsxd_rr(A, A);
            f.push_from_ty(A, ValType::I64);
        }
        I32Extend8S => {
            f.pop_to(A);
            f.asm.movsx8_rr(false, A, A);
            f.push_from_ty(A, ValType::I32);
        }
        I32Extend16S => {
            f.pop_to(A);
            f.asm.movsx16_rr(false, A, A);
            f.push_from_ty(A, ValType::I32);
        }
        I64Extend8S => {
            f.pop_to(A);
            f.asm.movsx8_rr(true, A, A);
            f.push_from_ty(A, ValType::I64);
        }
        I64Extend16S => {
            f.pop_to(A);
            f.asm.movsx16_rr(true, A, A);
            f.push_from_ty(A, ValType::I64);
        }
        I64Extend32S => {
            f.pop_to(A);
            f.asm.movsxd_rr(A, A);
            f.push_from_ty(A, ValType::I64);
        }

        // --- floating point ---
        F32Const { value } => f.push_imm(value.bits() as i64, ValType::F32),
        F64Const { value } => f.push_imm(value.bits() as i64, ValType::F64),

        F32Add => fbin(f, Fw::Single, |a, w, x, y| a.fadd(w, x, y)),
        F32Sub => fbin(f, Fw::Single, |a, w, x, y| a.fsub(w, x, y)),
        F32Mul => fbin(f, Fw::Single, |a, w, x, y| a.fmul(w, x, y)),
        F32Div => fbin(f, Fw::Single, |a, w, x, y| a.fdiv(w, x, y)),
        F64Add => fbin(f, Fw::Double, |a, w, x, y| a.fadd(w, x, y)),
        F64Sub => fbin(f, Fw::Double, |a, w, x, y| a.fsub(w, x, y)),
        F64Mul => fbin(f, Fw::Double, |a, w, x, y| a.fmul(w, x, y)),
        F64Div => fbin(f, Fw::Double, |a, w, x, y| a.fdiv(w, x, y)),

        F32Min => fminmax(f, Fw::Single, true),
        F32Max => fminmax(f, Fw::Single, false),
        F64Min => fminmax(f, Fw::Double, true),
        F64Max => fminmax(f, Fw::Double, false),

        F32Sqrt => funary(f, Fw::Single, |a, w, x| a.fsqrt(w, x, x)),
        F64Sqrt => funary(f, Fw::Double, |a, w, x| a.fsqrt(w, x, x)),
        F32Floor => funary(f, Fw::Single, |a, w, x| a.fround(w, 1, x, x)),
        F64Floor => funary(f, Fw::Double, |a, w, x| a.fround(w, 1, x, x)),
        F32Ceil => funary(f, Fw::Single, |a, w, x| a.fround(w, 2, x, x)),
        F64Ceil => funary(f, Fw::Double, |a, w, x| a.fround(w, 2, x, x)),
        F32Trunc => funary(f, Fw::Single, |a, w, x| a.fround(w, 3, x, x)),
        F64Trunc => funary(f, Fw::Double, |a, w, x| a.fround(w, 3, x, x)),
        // wasm rounds halves to even, which is `round`'s mode 0.
        F32Nearest => funary(f, Fw::Single, |a, w, x| a.fround(w, 0, x, x)),
        F64Nearest => funary(f, Fw::Double, |a, w, x| a.fround(w, 0, x, x)),

        // Sign work is bit work: clear the sign bit, flip it, or take it from
        // the other operand. No arithmetic, so NaNs pass through untouched —
        // which is exactly what wasm specifies for these three.
        F32Abs => fsign(f, Fw::Single, Sign::Abs),
        F64Abs => fsign(f, Fw::Double, Sign::Abs),
        F32Neg => fsign(f, Fw::Single, Sign::Neg),
        F64Neg => fsign(f, Fw::Double, Sign::Neg),
        F32Copysign => fsign(f, Fw::Single, Sign::Copy),
        F64Copysign => fsign(f, Fw::Double, Sign::Copy),

        F32Eq => fcmp(f, Fw::Single, FCmp::Eq),
        F32Ne => fcmp(f, Fw::Single, FCmp::Ne),
        F32Lt => fcmp(f, Fw::Single, FCmp::Lt),
        F32Gt => fcmp(f, Fw::Single, FCmp::Gt),
        F32Le => fcmp(f, Fw::Single, FCmp::Le),
        F32Ge => fcmp(f, Fw::Single, FCmp::Ge),
        F64Eq => fcmp(f, Fw::Double, FCmp::Eq),
        F64Ne => fcmp(f, Fw::Double, FCmp::Ne),
        F64Lt => fcmp(f, Fw::Double, FCmp::Lt),
        F64Gt => fcmp(f, Fw::Double, FCmp::Gt),
        F64Le => fcmp(f, Fw::Double, FCmp::Le),
        F64Ge => fcmp(f, Fw::Double, FCmp::Ge),

        F32DemoteF64 => {
            f.fpop_to(FA);
            f.asm.fconvert(Fw::Double, FA, FA);
            f.fpush_from(FA, ValType::F32);
        }
        F64PromoteF32 => {
            f.fpop_to(FA);
            f.asm.fconvert(Fw::Single, FA, FA);
            f.fpush_from(FA, ValType::F64);
        }

        // Signed int to float is one instruction. Unsigned from i32 is too,
        // by widening to i64 first — a u32 always fits. Unsigned from i64 is
        // the only one that needs work, below.
        F32ConvertI32S => int_to_f(f, Fw::Single, false),
        F64ConvertI32S => int_to_f(f, Fw::Double, false),
        F32ConvertI64S => int_to_f(f, Fw::Single, true),
        F64ConvertI64S => int_to_f(f, Fw::Double, true),
        F32ConvertI32U => {
            f.pop_to(A); // the 32-bit load already zero-extended
            f.asm.int_to_float(Fw::Single, true, FA, A);
            f.fpush_from(FA, ValType::F32);
        }
        F64ConvertI32U => {
            f.pop_to(A);
            f.asm.int_to_float(Fw::Double, true, FA, A);
            f.fpush_from(FA, ValType::F64);
        }
        F32ConvertI64U => u64_to_f(f, Fw::Single),
        F64ConvertI64U => u64_to_f(f, Fw::Double),

        I32TruncSatF32S => trunc_sat_s(f, Fw::Single, false),
        I32TruncSatF64S => trunc_sat_s(f, Fw::Double, false),
        I64TruncSatF32S => trunc_sat_s(f, Fw::Single, true),
        I64TruncSatF64S => trunc_sat_s(f, Fw::Double, true),
        I32TruncSatF32U => trunc_sat_u32(f, Fw::Single),
        I32TruncSatF64U => trunc_sat_u32(f, Fw::Double),

        I32ReinterpretF32 => {
            f.fpop_to(FA);
            f.asm.xmm_to_gpr(false, A, FA);
            f.push_from_ty(A, ValType::I32);
        }
        I64ReinterpretF64 => {
            f.fpop_to(FA);
            f.asm.xmm_to_gpr(true, A, FA);
            f.push_from_ty(A, ValType::I64);
        }
        F32ReinterpretI32 => {
            f.pop_to(A);
            f.asm.gpr_to_xmm(false, FA, A);
            f.fpush_from(FA, ValType::F32);
        }
        F64ReinterpretI64 => {
            f.pop_to(A);
            f.asm.gpr_to_xmm(true, FA, A);
            f.fpush_from(FA, ValType::F64);
        }

        // The only operator that cannot be done with instructions alone: it
        // needs a mapping changed. So it calls the one runtime routine, and
        // afterwards the pinned memory base MUST be reloaded — this is the
        // case the invariant on MEMBASE was written for.
        MemoryGrow { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            f.spill_all();
            f.pop_to(A);
            f.asm.mov_rr64(Reg::Rsi, A);
            f.asm.mov_rr64(Reg::Rdi, VMCTX);
            f.asm.load64(T, VMCTX, vmctx::BUILTIN_GROW);
            f.asm.call_reg(T);
            f.push_from_ty(A, ValType::I32);
        }
        MemorySize { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            f.asm.load64(A, VMCTX, vmctx::MEM_SIZE);
            f.asm.shr_imm(true, A, 16); // bytes to 64 KiB pages
            f.push_from_ty(A, ValType::I32);
        }

        // --- bulk memory ---
        //
        // The first operators where the guard page is NOT enough. A guard
        // catches one access; these walk a RANGE, and the specification wants
        // the trap BEFORE the first byte moves. Faulting halfway would already
        // have changed memory — observably different from the interpreter.
        // So the bounds are checked up front, in 64 bits, where `dst + len`
        // cannot overflow because both halves are u32.
        MemoryCopy { dst_mem, src_mem } => {
            if *dst_mem != 0 || *src_mem != 0 {
                return Err("multi-memory");
            }
            // `rep movsb` wants rsi/rdi/rcx, and two of those are cache
            // registers — so the cache goes to memory first.
            f.spill_all();
            f.pop_to(B); // rcx = len
            f.pop_to(Reg::Rsi); // src
            f.pop_to(Reg::Rdi); // dst
            bulk_bounds(f, Reg::Rdi)?;
            bulk_bounds(f, Reg::Rsi)?;
            f.asm.add_rr64(Reg::Rsi, MEMBASE);
            f.asm.add_rr64(Reg::Rdi, MEMBASE);

            // wasm's memory.copy is a memmove: overlapping ranges must come
            // out right. Copying downwards is safe whenever the destination
            // is at or below the source; otherwise the copy has to run
            // backwards, which is what the direction flag is for.
            f.asm.cmp_rr64(Reg::Rdi, Reg::Rsi);
            let forward = f.asm.jcc(Cond::Be);
            f.asm.add_rr64(Reg::Rsi, B);
            f.asm.sub_r64_imm32(Reg::Rsi, 1);
            f.asm.add_rr64(Reg::Rdi, B);
            f.asm.sub_r64_imm32(Reg::Rdi, 1);
            f.asm.std();
            f.asm.rep_movsb();
            f.asm.cld();
            let done = f.asm.jmp();
            f.asm.bind(forward);
            f.asm.rep_movsb();
            f.asm.bind(done);
        }
        MemoryFill { mem } => {
            if *mem != 0 {
                return Err("multi-memory");
            }
            f.spill_all();
            f.pop_to(B); // rcx = len
            f.pop_to(A); // al = the byte
            f.pop_to(Reg::Rdi); // dst
            bulk_bounds(f, Reg::Rdi)?;
            f.asm.add_rr64(Reg::Rdi, MEMBASE);
            f.asm.rep_stosb();
        }

        // --- calls ---
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

        // --- linear memory ---
        //
        // No bounds-check instruction is emitted, and that is the design, not
        // an omission. The instance reserves 8 GiB of address space and maps
        // only the pages that exist. A wasm address is a u32 and the offset is
        // a u32, so `base + zext(addr) + offset` cannot reach past
        // `base + 8 GiB` — every address outside the memory lands on unmapped
        // ground and faults. The fault must become a module trap rather than a
        // kernel panic; that is the page-fault handler's job, and it is the
        // one piece this depends on.
        // Floats ride the same guard-page reservation; only the register
        // file differs.
        F32Load { memarg } => fload(f, memarg, ValType::F32)?,
        F64Load { memarg } => fload(f, memarg, ValType::F64)?,
        F32Store { memarg } => fstore(f, memarg, Fw::Single)?,
        F64Store { memarg } => fstore(f, memarg, Fw::Double)?,

        I32Load { memarg } => load(f, memarg, ValType::I32, |a, d, b, i, o| a.load32_idx(d, b, i, o))?,
        I32Load8U { memarg } => load(f, memarg, ValType::I32, |a, d, b, i, o| a.load8u_idx(d, b, i, o))?,
        I32Load8S { memarg } => load(f, memarg, ValType::I32, |a, d, b, i, o| a.load8s_idx(false, d, b, i, o))?,
        I32Load16U { memarg } => load(f, memarg, ValType::I32, |a, d, b, i, o| a.load16u_idx(d, b, i, o))?,
        I32Load16S { memarg } => load(f, memarg, ValType::I32, |a, d, b, i, o| a.load16s_idx(false, d, b, i, o))?,

        I64Load { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load64_idx(d, b, i, o))?,
        // A zero-extending narrow load clears the whole register, so the
        // 32-bit form serves i64 unchanged. The sign-extending ones do not:
        // filling 64 bits with the sign is a different instruction.
        I64Load8U { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load8u_idx(d, b, i, o))?,
        I64Load16U { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load16u_idx(d, b, i, o))?,
        I64Load32U { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load32_idx(d, b, i, o))?,
        I64Load8S { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load8s_idx(true, d, b, i, o))?,
        I64Load16S { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load16s_idx(true, d, b, i, o))?,
        I64Load32S { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load32s_idx(d, b, i, o))?,

        I32Store { memarg } => store(f, memarg, |a, b, i, o, s| a.store32_idx(b, i, o, s))?,
        I32Store8 { memarg } => store(f, memarg, |a, b, i, o, s| a.store8_idx(b, i, o, s))?,
        I32Store16 { memarg } => store(f, memarg, |a, b, i, o, s| a.store16_idx(b, i, o, s))?,
        // A narrow i64 store writes the low bytes of the register, which is
        // the very same instruction the i32 ones use.
        I64Store { memarg } => store(f, memarg, |a, b, i, o, s| a.store64_idx(b, i, o, s))?,
        I64Store8 { memarg } => store(f, memarg, |a, b, i, o, s| a.store8_idx(b, i, o, s))?,
        I64Store16 { memarg } => store(f, memarg, |a, b, i, o, s| a.store16_idx(b, i, o, s))?,
        I64Store32 { memarg } => store(f, memarg, |a, b, i, o, s| a.store32_idx(b, i, o, s))?,

        // `select` takes the condition last, so the two candidates sit below
        // it. `cmov` reads the flags the condition's `test` left, and neither
        // `mov` in between disturbs them.
        Select => {
            f.pop_to(A); // the condition
            let Some(t) = f.peek_ty() else {
                return Err("select-empty");
            };
            if !int_ty(t) {
                return Err("select-type");
            }
            f.asm.test32(A, A);
            f.free_gpr(A);
            // Only bits are being chosen, so `cmov` on general registers
            // serves floats as well — and avoids a branch on a condition that
            // is usually unpredictable.
            // Nothing between the `test` and the `cmov` touches the flags:
            // moves, loads and stores all leave them alone.
            let w = wide(t);
            f.pop_to(B); // the value taken when the condition is false
            f.pop_to(A); // the value taken when it is true
            // Zero means false, so `cmovz` is what picks the second value —
            // which is why the first one is fetched into the destination.
            f.asm.cmov(w, Cond::E, A, B);
            f.free_gpr(B);
            f.push_from_ty(A, t);
        }


        other => return Err(crate::names::op_name(other)),
    }
    Ok(())
}

/// Move the branch's values into the target frame's slots and jump. A loop's
/// label is its head; every other frame's label is its end.
fn branch_to(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {
    let idx = f
        .ctrl
        .len()
        .checked_sub(1 + relative_depth as usize)
        .ok_or("br-depth")?;
    let arity = f.ctrl[idx].branch_arity;
    let height = f.ctrl[idx].height;

    if arity == 1 {
        let t = f.peek_ty().unwrap_or(ValType::I32);
        f.peek_to(A);
        let dst = f.slot(height);
        if wide(t) {
            f.asm.store64(Reg::Rbp, dst, A);
        } else {
            f.asm.store32(Reg::Rbp, dst, A);
        }
    }
    if f.ctrl[idx].kind == Kind::Loop {
        let start = f.ctrl[idx].start;
        f.asm.jmp_back(start);
    } else {
        let p = f.asm.jmp();
        f.ctrl[idx].ends.push(p);
    }
    Ok(())
}

/// A jump table, not a chain of compares: `br_table` is how a `switch`
/// arrives, and a chain would make the cost depend on which case was taken.
///
/// Layout is dispatch, then one stub per case, then the table itself. The
/// table sits last because every stub ends in a jump, so control can never
/// fall into it — and putting it there means no jump has to skip over it.
fn br_table(f: &mut Ctx, t: &wasmparser::BrTable<'_>) -> Result<(), &'static str> {
    let n = t.len() as usize;
    let mut labels: Vec<u32> = Vec::with_capacity(n);
    for x in t.targets() {
        labels.push(x.map_err(|_| "brtable-read")?);
    }
    let default = t.default();

    f.pop_to(B);
    f.asm.cmp_r32_imm32(B, n as i32);
    let to_default = f.asm.jcc(Cond::Ae);

    let lea_at = f.asm.lea_rip_blank(T);
    f.asm.movsxd_scaled(A, T, B, 2, 0);
    f.asm.add_rr64(A, T);
    f.asm.jmp_reg(A);

    // Every case gets a stub, because a branch may have to move a value into
    // its label's slot before jumping, and that move differs per target.
    let mut stubs = Vec::with_capacity(n);
    for &d in &labels {
        stubs.push(f.asm.pos());
        branch_to(f, d)?;
    }
    f.asm.bind(to_default);
    branch_to(f, default)?;

    let table = f.asm.pos();
    f.asm
        .patch_i32(lea_at, (table as i64 - (lea_at as i64 + 4)) as i32);
    for p in stubs {
        f.asm.emit_i32((p as i64 - table as i64) as i32);
    }
    Ok(())
}

/// `br_if` cannot move the branch's value before the test: the value's
/// destination slot may still hold a live operand on the fall-through path.
/// With no values to carry it is a single conditional jump; otherwise the
/// move sits on the taken side of a short skip.
fn br_if(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {
    let idx = f
        .ctrl
        .len()
        .checked_sub(1 + relative_depth as usize)
        .ok_or("br-depth")?;
    let arity = f.ctrl[idx].branch_arity;

    f.pop_to(A);
    // The taken side lands on a label, and the fall-through continues from
    // here — both have to agree, so settle before either happens.
    f.spill_all();
    f.asm.test32(A, A);

    if arity == 0 {
        if f.ctrl[idx].kind == Kind::Loop {
            let start = f.ctrl[idx].start;
            f.asm.jcc_back(Cond::Ne, start);
        } else {
            let p = f.asm.jcc(Cond::Ne);
            f.ctrl[idx].ends.push(p);
        }
        return Ok(());
    }

    let skip = f.asm.jcc(Cond::E);
    branch_to(f, relative_depth)?;
    f.asm.bind(skip);
    Ok(())
}

/// `start + len > memory size` traps, where `start` is a zero-extended wasm
/// address in `reg` and `len` sits in `B`. Both are u32, so the sum is at most
/// 2^33-2 and the 64-bit addition cannot wrap — the check is exact rather than
/// merely conservative.
///
/// A zero length still traps when the start is past the end, which is what the
/// specification says and what the interpreter does.
fn bulk_bounds(f: &mut Ctx, reg: Reg) -> Result<(), &'static str> {
    f.asm.load64(C, VMCTX, vmctx::MEM_SIZE);
    f.asm.mov_rr64(T, reg);
    f.asm.add_rr64(T, B);
    f.asm.cmp_rr64(T, C);
    f.trap_if(Cond::A, trap::MEMORY_OUT_OF_BOUNDS);
    Ok(())
}

/// Signature of a callee, refused unless it fits the slice we can generate.
fn callee_shape<'a>(
    m: &'a ModuleCtx<'a>,
    type_index: u32,
) -> Result<(&'a [ValType], &'a [ValType]), &'static str> {
    let (p, r) = m.types.get(type_index as usize).ok_or("callee-type")?;
    if !p.iter().copied().all(int_ty) {
        return Err("callee-param-type");
    }
    if r.len() > 1 || !r.iter().copied().all(int_ty) {
        return Err("callee-result-type");
    }
    Ok((p, r))
}

/// Arguments sit on the operand stack, deepest first. They go straight from
/// their frame slots into the argument registers — the destinations are
/// distinct and the sources are memory, so the order is free.
///
/// Returns the bytes reserved below `rsp` for the arguments that did not fit
/// in registers; the caller gives them back after the call. Moving `rsp` per
/// call rather than reserving an outgoing area in the frame keeps every slot
/// displacement known at emit time — a single pass has no chance to revisit
/// them, and calls with more than five arguments are rare.
fn load_args(f: &mut Ctx, params: &[ValType]) -> i32 {
    let n = params.len();
    let base = f.depth() as usize - n;
    let (places, n_stack) = arg_places(params);
    let bytes = outgoing_bytes(n_stack);

    // The stack arguments go down first, while `rax` is still free; the
    // register ones follow, because loading them is what finally commits the
    // argument registers.
    if bytes > 0 {
        f.asm.sub_r64_imm32(Reg::Rsp, bytes);
        for (i, t) in params.iter().enumerate() {
            let Place::Stack(j) = places[i] else { continue };
            let off = f.slot((base + i) as u32);
            if wide(*t) {
                f.asm.load64(A, Reg::Rbp, off);
            } else {
                // The 32-bit load zero-extends, so the whole eight-byte slot
                // is defined and a native callee sees no rubbish above the
                // value.
                f.asm.load32(A, Reg::Rbp, off);
            }
            f.asm.store64(Reg::Rsp, 8 * j as i32, A);
        }
    }
    for (i, t) in params.iter().enumerate() {
        let off = f.slot((base + i) as u32);
        match places[i] {
            Place::Int(k) => {
                if wide(*t) {
                    f.asm.load64(ARG_REGS[k], Reg::Rbp, off);
                } else {
                    f.asm.load32(ARG_REGS[k], Reg::Rbp, off);
                }
            }
            Place::Flt(k) => f.asm.fload_slot(fw_of(*t), FARG_REGS[k], Reg::Rbp, off),
            Place::Stack(_) => {}
        }
    }
    bytes
}

/// After a call there is nothing to repair. The instance reserves its address
/// space once and only ever makes more of it readable, so **the memory base
/// never moves** — not across a call, not across `memory.grow`. That falls out
/// of the guard-page design rather than being an extra promise, and it is what
/// lets the base stay pinned without a reload per call.
fn after_call(f: &mut Ctx, n_args: usize, results: &[ValType], arg_bytes: i32) {
    if arg_bytes > 0 {
        f.asm.add_r64_imm32(Reg::Rsp, arg_bytes);
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
    // Before anything else. Nothing in a register survives a call — and a
    // spill materialises through `r11`, which the indirect path below is about
    // to hold the call target in.
    f.spill_all();
    let ti = *f
        .m
        .func_type_of
        .get(function_index as usize)
        .ok_or("callee-index")?;
    let (params, results) = callee_shape(f.m, ti)?;
    let n = params.len();

    let arg_bytes = load_args(f, params);

    if function_index < f.m.n_imported {
        // An import is a native function: it expects the context as its first
        // argument the ordinary way. A wasm callee reads it out of the pinned
        // register instead, so nothing has to be set up for one.
        f.asm.mov_rr64(Reg::Rdi, VMCTX);
        // A native function may want to read the counter, or charge against
        // it for work of its own, so it is handed over and taken back.
        f.asm.store64(VMCTX, vmctx::FUEL, FUEL);
        f.asm.load64(T, VMCTX, vmctx::HOST_FNS);
        f.asm.load64(T, T, 8 * function_index as i32);
        f.asm.call_reg(T);
        f.asm.load64(FUEL, VMCTX, vmctx::FUEL);
    } else {
        let at = f.asm.call_rel32_blank();
        f.relocs.push(Reloc {
            at,
            target: function_index,
        });
    }
    after_call(f, n, results, arg_bytes);
    Ok(())
}

/// The one place a bounds check really is needed: a table index cannot be
/// covered by a guard page. The signature check compares CANONICAL ids, not
/// type indices — wasm types are structural, and comparing indices would
/// reject calls the spec allows.
fn call_indirect(f: &mut Ctx, type_index: u32) -> Result<(), &'static str> {
    // First, for the same reason as the direct path: the target lands in
    // `r11` further down, and that is the register a spill borrows.
    f.spill_all();
    let (params, results) = callee_shape(f.m, type_index)?;
    let n = params.len();
    let want = *f.m.sig_id.get(type_index as usize).ok_or("callee-type")?;

    f.pop_to(B); // table index, zero-extended by the 32-bit move
    f.asm.load64(C, VMCTX, vmctx::TABLE_LEN);
    f.asm.cmp_rr64(B, C);
    f.trap_if(Cond::Ae, trap::TABLE_OUT_OF_BOUNDS);

    f.asm.load64(T, VMCTX, vmctx::TABLE_SIGS);
    f.asm.load32_scaled(T, T, B, 2, 0);
    f.asm.cmp_r32_imm32(T, want as i32);
    f.trap_if(Cond::Ne, trap::BAD_SIGNATURE);

    // The target must be in hand before the arguments are loaded: loading
    // them writes over `rcx` and `rdx`.
    f.asm.load64(T, VMCTX, vmctx::TABLE);
    f.asm.load64_scaled(T, T, B, 3, 0);

    let arg_bytes = load_args(f, params);
    f.asm.call_reg(T);
    after_call(f, n, results, arg_bytes);
    Ok(())
}

/// Turn a memory immediate into a displacement, folding an oversized offset
/// into the address register. `disp32` is SIGNED, so an offset above 2 GiB
/// cannot be encoded — and since both the address and the offset are u32, the
/// sum still fits inside the 8 GiB reservation, so folding is safe.
fn mem_disp(
    f: &mut Ctx,
    memarg: &wasmparser::MemArg,
    idx: Reg,
    scratch: Reg,
) -> Result<i32, &'static str> {
    if memarg.memory != 0 {
        return Err("multi-memory");
    }
    let off = memarg.offset;
    if off <= i32::MAX as u64 {
        Ok(off as i32)
    } else if off <= u32::MAX as u64 {
        f.asm.mov_r64_imm64(scratch, off as i64);
        f.asm.add_rr64(idx, scratch);
        Ok(0)
    } else {
        // Only reachable with memory64, which `features()` does not admit.
        Err("offset>4G")
    }
}

/// `addr = pop; push(mem[addr + offset])`. `pop_to` is a 32-bit move, so the
/// address register already holds the zero-extended wasm address — exactly
/// what the reservation's arithmetic needs.
fn load(
    f: &mut Ctx,
    memarg: &wasmparser::MemArg,
    ty: ValType,
    mut go: impl FnMut(&mut Asm, Reg, Reg, Reg, i32),
) -> Result<(), &'static str> {
    f.pop_to(B);
    let disp = mem_disp(f, memarg, B, C)?;
    // Straight into a cache register: a loaded value is almost always used
    // right away, and a move in between would be pure loss.
    let d = f.alloc_gpr();
    go(&mut f.asm, d, MEMBASE, B, disp);
    f.push_reg(d, ty);
    Ok(())
}

/// `value = pop; addr = pop; mem[addr + offset] = value` — wasm pushes the
/// address first, so the value comes off the stack first.
fn store(
    f: &mut Ctx,
    memarg: &wasmparser::MemArg,
    mut go: impl FnMut(&mut Asm, Reg, Reg, i32, Reg),
) -> Result<(), &'static str> {
    f.pop_to(A);
    f.pop_to(B);
    let disp = mem_disp(f, memarg, B, C)?;
    go(&mut f.asm, MEMBASE, B, disp, A);
    Ok(())
}

/// Which way `fsign` bends the sign bit.
enum Sign {
    Abs,
    Neg,
    Copy,
}

/// How a float comparison maps onto `ucomis*`, whose unordered case sets
/// CF, ZF and PF all at once.
enum FCmp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

/// The sign-bit mask for a width, as an immediate.
fn sign_mask(fw: Fw) -> i64 {
    if fw.is_double() { i64::MIN } else { 0x8000_0000 }
}

fn fload(f: &mut Ctx, memarg: &wasmparser::MemArg, ty: ValType) -> Result<(), &'static str> {
    f.pop_to(B);
    let disp = mem_disp(f, memarg, B, C)?;
    f.asm.fload_idx(fw_of(ty), FA, MEMBASE, B, disp);
    f.fpush_from(FA, ty);
    Ok(())
}

fn fstore(f: &mut Ctx, memarg: &wasmparser::MemArg, fw: Fw) -> Result<(), &'static str> {
    f.fpop_to(FA);
    f.pop_to(B);
    let disp = mem_disp(f, memarg, B, C)?;
    f.asm.fstore_idx(fw, MEMBASE, B, disp, FA);
    Ok(())
}

fn fbin(f: &mut Ctx, fw: Fw, mut go: impl FnMut(&mut Asm, Fw, Xmm, Xmm)) {
    f.fpop_to(FB);
    // The left operand is worked on where it lies and stays there.
    let d = f.fpop_to_pool();
    go(&mut f.asm, fw, d, FB);
    f.fpush_reg(d, if fw.is_double() { ValType::F64 } else { ValType::F32 });
}

fn funary(f: &mut Ctx, fw: Fw, mut go: impl FnMut(&mut Asm, Fw, Xmm)) {
    let d = f.fpop_to_pool();
    go(&mut f.asm, fw, d);
    f.fpush_reg(d, if fw.is_double() { ValType::F64 } else { ValType::F32 });
}

/// `abs`, `neg` and `copysign` are bit operations, not arithmetic — so a NaN
/// operand comes out with its payload intact, which is what wasm asks for and
/// what an arithmetic lowering would quietly destroy.
fn fsign(f: &mut Ctx, fw: Fw, kind: Sign) {
    let ty = if fw.is_double() { ValType::F64 } else { ValType::F32 };
    let mask = sign_mask(fw);
    match kind {
        Sign::Abs => {
            f.fpop_to(FA);
            f.asm.mov_r64_imm64(A, mask);
            f.asm.gpr_to_xmm(true, FB, A);
            // Clear the sign: keep everything the mask does NOT cover.
            f.asm.fandn(fw, FB, FA);
            f.asm.fmov(FA, FB);
        }
        Sign::Neg => {
            f.fpop_to(FA);
            f.asm.mov_r64_imm64(A, mask);
            f.asm.gpr_to_xmm(true, FB, A);
            f.asm.fxor(fw, FA, FB);
        }
        Sign::Copy => {
            f.fpop_to(FB); // the sign donor
            f.fpop_to(FA); // the magnitude
            f.asm.mov_r64_imm64(A, mask);
            f.asm.gpr_to_xmm(true, FC, A);
            f.asm.fand(fw, FB, FC); // just the donor's sign
            f.asm.fandn(fw, FC, FA); // the magnitude without its own sign
            f.asm.f_or(fw, FC, FB);
            f.asm.fmov(FA, FC);
        }
    }
    f.fpush_from(FA, ty);
}

/// wasm's `min`/`max` differ from the hardware's in two places, and both
/// matter: with a NaN operand wasm wants a NaN but `minss` returns its second
/// source, and `min(+0,-0)` must be `-0` while `minss` again just returns the
/// second source. So the three cases are separated by hand — unordered,
/// equal, and the ordinary case where the hardware instruction is right.
fn fminmax(f: &mut Ctx, fw: Fw, is_min: bool) {
    let ty = if fw.is_double() { ValType::F64 } else { ValType::F32 };
    f.fpop_to(FB);
    f.fpop_to(FA);
    f.asm.fucomi(fw, FA, FB);
    let nan = f.asm.jcc(Cond::P);
    let equal = f.asm.jcc(Cond::E);
    if is_min {
        f.asm.fmin_raw(fw, FA, FB);
    } else {
        f.asm.fmax_raw(fw, FA, FB);
    }
    let done1 = f.asm.jmp();

    // Equal, so the only question left is the sign of a zero. Or of the two
    // sign bits gives -0, and of them gives +0 — exactly min and max.
    f.asm.bind(equal);
    if is_min {
        f.asm.f_or(fw, FA, FB);
    } else {
        f.asm.fand(fw, FA, FB);
    }
    let done2 = f.asm.jmp();

    // Unordered: adding propagates the NaN and quiets it. wasm leaves the
    // payload to the implementation, so any NaN will do.
    f.asm.bind(nan);
    f.asm.fadd(fw, FA, FB);

    f.asm.bind(done1);
    f.asm.bind(done2);
    f.fpush_from(FA, ty);
}

/// `ucomis*` sets CF, ZF and PF together, and unordered sets all three. So
/// `setb` would call a NaN "less than", which wasm forbids: every ordered
/// comparison has to be phrased with `seta`/`setae`, which need CF clear.
/// The two operands are swapped where that phrasing requires it.
fn fcmp(f: &mut Ctx, fw: Fw, kind: FCmp) {
    f.fpop_to(FB);
    f.fpop_to(FA);
    match kind {
        FCmp::Eq => {
            f.asm.fucomi(fw, FA, FB);
            f.asm.set_cond(Cond::E, A);
            f.asm.set_cond(Cond::Np, B);
            f.asm.and32(A, B); // equal AND ordered
        }
        FCmp::Ne => {
            f.asm.fucomi(fw, FA, FB);
            f.asm.set_cond(Cond::Ne, A);
            f.asm.set_cond(Cond::P, B);
            f.asm.or32(A, B); // different OR unordered — `ne` is the one
                              // comparison that is TRUE for a NaN
        }
        FCmp::Gt => {
            f.asm.fucomi(fw, FA, FB);
            f.asm.set_cond(Cond::A, A);
        }
        FCmp::Ge => {
            f.asm.fucomi(fw, FA, FB);
            f.asm.set_cond(Cond::Ae, A);
        }
        FCmp::Lt => {
            f.asm.fucomi(fw, FB, FA);
            f.asm.set_cond(Cond::A, A);
        }
        FCmp::Le => {
            f.asm.fucomi(fw, FB, FA);
            f.asm.set_cond(Cond::Ae, A);
        }
    }
    f.push_from_ty(A, ValType::I32);
}

fn int_to_f(f: &mut Ctx, fw: Fw, w: bool) {
    f.pop_to(A);
    f.asm.int_to_float(fw, w, FA, A);
    f.fpush_from(FA, if fw.is_double() { ValType::F64 } else { ValType::F32 });
}

/// u64 to float. x86 only converts SIGNED integers, so a value with the top
/// bit set has to be halved first, converted, and doubled back. Halving with
/// a plain shift would throw away the lowest bit and round wrong, so the lost
/// bit is folded back in — round-to-odd — before the conversion.
fn u64_to_f(f: &mut Ctx, fw: Fw) {
    let ty = if fw.is_double() { ValType::F64 } else { ValType::F32 };
    f.pop_to(A);
    f.asm.test(true, A, A);
    let negative = f.asm.jcc(Cond::S);
    f.asm.int_to_float(fw, true, FA, A);
    let done = f.asm.jmp();

    f.asm.bind(negative);
    f.asm.mov_rr64(B, A);
    f.asm.shr_imm(true, B, 1);
    f.asm.and_r_imm32(true, A, 1);
    f.asm.or(true, B, A);
    f.asm.int_to_float(fw, true, FA, B);
    f.asm.fadd(fw, FA, FA);

    f.asm.bind(done);
    f.fpush_from(FA, ty);
}

/// Signed saturating truncation. `cvtt*2si` answers with the "integer
/// indefinite" value — the minimum — for a NaN, for either overflow, AND for
/// a legitimate minimum. wasm wants 0 for the NaN, the maximum for a positive
/// overflow, and the minimum for the other two, so the ambiguous answer has
/// to be taken apart afterwards. It is the rare path, so it sits behind the
/// branch rather than in front of it.
fn trunc_sat_s(f: &mut Ctx, fw: Fw, w: bool) {
    let ty = if w { ValType::I64 } else { ValType::I32 };
    let (min, max) = if w {
        (i64::MIN, i64::MAX)
    } else {
        (i32::MIN as i64, i32::MAX as i64)
    };
    f.fpop_to(FA);
    f.asm.float_to_int(fw, w, A, FA);
    // The indefinite value is the width's own minimum, and in 64 bits that
    // does not fit an immediate — a sign-extended `imm32` would compare
    // against 0xFFFFFFFF80000000 instead, and every conversion would look
    // ordinary.
    if w {
        f.asm.mov_r64_imm64(B, i64::MIN);
        f.asm.cmp_rr64(A, B);
    } else {
        f.asm.cmp_r32_imm32(A, i32::MIN);
    }
    let done_early = f.asm.jcc(Cond::Ne);

    f.asm.xor(false, A, A); // a NaN answers zero
    f.asm.fucomi(fw, FA, FA);
    let done_nan = f.asm.jcc(Cond::P);

    f.asm.mov_r64_imm64(A, min);
    f.asm.fxor(fw, FB, FB); // 0.0
    f.asm.fucomi(fw, FA, FB);
    let done_neg = f.asm.jcc(Cond::Be);
    f.asm.mov_r64_imm64(A, max);

    f.asm.bind(done_early);
    f.asm.bind(done_nan);
    f.asm.bind(done_neg);
    f.push_from_ty(A, ty);
}

/// Unsigned saturating truncation to i32. There is no unsigned convert, but
/// the whole u32 range fits comfortably inside a signed i64 — so the range is
/// fenced off first and the conversion then runs in 64 bits, where it cannot
/// go indefinite.
fn trunc_sat_u32(f: &mut Ctx, fw: Fw) {
    // 2^32, as the bits of the respective float.
    let two32 = if fw.is_double() { 0x41F0_0000_0000_0000u64 } else { 0x4F80_0000 };
    f.fpop_to(FA);
    f.asm.xor(false, A, A); // NaN and everything at or below zero answer 0
    f.asm.fucomi(fw, FA, FA);
    let done_nan = f.asm.jcc(Cond::P);
    f.asm.fxor(fw, FB, FB);
    f.asm.fucomi(fw, FA, FB);
    let done_neg = f.asm.jcc(Cond::Be);

    f.asm.mov_r64_imm64(B, two32 as i64);
    f.asm.gpr_to_xmm(true, FB, B);
    f.asm.fucomi(fw, FA, FB);
    f.asm.mov_r64_imm64(A, 0xFFFF_FFFF); // saturates; `mov` leaves flags alone
    let done_big = f.asm.jcc(Cond::Ae);
    f.asm.float_to_int(fw, true, A, FA);

    f.asm.bind(done_nan);
    f.asm.bind(done_neg);
    f.asm.bind(done_big);
    f.push_from_ty(A, ValType::I32);
}

/// One ALU operation in its three encodings: against a register, against
/// memory, and against an immediate. Having all three lets the right-hand
/// operand be used where it already is.
#[derive(Copy, Clone)]
struct AluOp {
    rr: u8,
    rm: u8,
    imm_ext: u8,
}

const ADD: AluOp = AluOp { rr: 0x01, rm: 0x03, imm_ext: 0 };
const SUB: AluOp = AluOp { rr: 0x29, rm: 0x2B, imm_ext: 5 };
const AND: AluOp = AluOp { rr: 0x21, rm: 0x23, imm_ext: 4 };
const OR: AluOp = AluOp { rr: 0x09, rm: 0x0B, imm_ext: 1 };
const XOR: AluOp = AluOp { rr: 0x31, rm: 0x33, imm_ext: 6 };
const CMP_OP: AluOp = AluOp { rr: 0x39, rm: 0x3B, imm_ext: 7 };

/// Where the right-hand operand can be reached without moving it.
enum Rhs {
    /// Fold it into the instruction as an immediate.
    Imm(i32),
    /// Address it in place — a local's slot or a spilled stack slot.
    Mem(i32),
    /// Nothing clever available; it went into `rcx`.
    Reg,
}

/// Take the right-hand operand off the stack WITHOUT materialising it, if it
/// is somewhere an instruction can reach directly. This is where most of the
/// remaining memory traffic goes away: `LocalGet` is 23,4 % of all
/// instructions and `I32Const` 17 %, and as a right-hand operand neither of
/// them needs a register at all.
fn take_rhs(f: &mut Ctx) -> Rhs {
    let Some(top) = f.stack.last().copied() else {
        return Rhs::Reg;
    };
    match top.loc {
        Loc::Imm(c) if i32::try_from(c).is_ok() => {
            f.stack.pop();
            Rhs::Imm(c as i32)
        }
        Loc::Local(idx) => {
            f.stack.pop();
            Rhs::Mem(f.local(idx))
        }
        Loc::Slot => {
            f.stack.pop();
            Rhs::Mem(f.slot(f.depth()))
        }
        _ => {
            f.pop_to(B);
            Rhs::Reg
        }
    }
}

/// `b = pop; a = pop; push(a OP b)` — wasm's operand order, so the first
/// popped value is the right-hand side and the direction of `sub` is kept.
fn bin(f: &mut Ctx, w: bool, op: AluOp) {
    let rhs = take_rhs(f);
    // The left-hand operand is worked on where it lies, and the result stays
    // there. No move in, no move out.
    let d = f.pop_to_pool();
    match rhs {
        Rhs::Imm(c) => f.asm.alu_r_imm32(w, op.imm_ext, d, c),
        Rhs::Mem(off) => f.asm.alu_rm(w, op.rm, d, Reg::Rbp, off),
        Rhs::Reg => f.asm.alu_raw(w, op.rr, d, B),
    }
    f.push_reg(d, if w { ValType::I64 } else { ValType::I32 });
}

fn mul(f: &mut Ctx, w: bool) {
    let rhs = take_rhs(f);
    let d = f.pop_to_pool();
    match rhs {
        Rhs::Imm(c) => f.asm.imul_r_imm32(w, d, d, c),
        Rhs::Mem(off) => f.asm.imul_rm(w, d, Reg::Rbp, off),
        Rhs::Reg => f.asm.imul(w, d, B),
    }
    f.push_reg(d, if w { ValType::I64 } else { ValType::I32 });
}

/// x86 takes a variable shift count from `cl` and nowhere else — but a
/// constant count is an immediate, and most counts in real code are constant.
fn shift(f: &mut Ctx, w: bool, ext: u8) {
    let imm = match f.stack.last().map(|v| v.loc) {
        Some(Loc::Imm(c)) => {
            f.stack.pop();
            Some((c & 63) as u8)
        }
        _ => {
            f.pop_to(B);
            None
        }
    };
    let d = f.pop_to_pool();
    match imm {
        Some(c) => f.asm.shift_imm(w, ext, d, c),
        None => f.asm.shift_cl_pub(w, ext, d),
    }
    f.push_reg(d, if w { ValType::I64 } else { ValType::I32 });
}

/// A comparison consumes operands of its own width and always leaves an i32.
fn cmp(f: &mut Ctx, w: bool, cc: Cond) {
    let rhs = take_rhs(f);
    let d = f.pop_to_pool();
    match rhs {
        Rhs::Imm(c) => f.asm.alu_r_imm32(w, CMP_OP.imm_ext, d, c),
        Rhs::Mem(off) => f.asm.alu_rm(w, CMP_OP.rm, d, Reg::Rbp, off),
        Rhs::Reg => f.asm.alu_raw(w, CMP_OP.rr, d, B),
    }
    f.asm.set_cond(cc, d);
    f.push_reg(d, ValType::I32);
}

/// Divide or take the remainder. The dividend has to be in the accumulator
/// and the high half has to be prepared, so the operand registers are not
/// free here the way they are elsewhere: `rax` takes the dividend, `rdx` is
/// clobbered as the high half, and the divisor goes in `rcx`.
fn div_op(f: &mut Ctx, w: bool, signed: bool, rem: bool) {
    f.pop_to(B); // divisor
    f.pop_to(A); // dividend
    let ty = if w { ValType::I64 } else { ValType::I32 };

    if signed && rem {
        // A divisor of -1 leaves remainder 0 for every dividend, INT_MIN
        // included — and that is the one case where wasm wants an answer
        // rather than a trap.
        f.asm.cmp_r_imm32(w, B, -1);
        let normal = f.asm.jcc(Cond::Ne);
        f.asm.xor(false, A, A);
        let done = f.asm.jmp();
        f.asm.bind(normal);
        f.asm.sign_extend_acc(w);
        f.asm.idiv(w, B);
        f.asm.mov_rr64(A, C);
        f.asm.bind(done);
    } else {
        if signed {
            f.asm.sign_extend_acc(w);
            f.asm.idiv(w, B);
        } else {
            // The high half must be zero, or `div` would work from rubbish
            // and could even overflow.
            f.asm.xor(false, C, C);
            f.asm.div(w, B);
        }
        if rem {
            f.asm.mov_rr64(A, C);
        }
    }
    f.push_from_ty(A, ty);
}

fn eqz(f: &mut Ctx, w: bool) {
    f.pop_to(A);
    f.asm.test(w, A, A);
    f.asm.set_cond(Cond::E, A);
    f.push_from_ty(A, ValType::I32);
}

/// `clz`: `bsr` gives the index of the highest set bit, so `width-1 - index`
/// is the answer — and `xor` with `width-1` computes that. The sentinel for a
/// zero input is picked so the SAME `xor` turns it into `width`:
/// `63 ^ 31 = 32`, `127 ^ 63 = 64`. `mov` and `cmov` leave flags alone, so
/// ZF from `bsr` still holds when the `cmov` reads it.
fn clz(f: &mut Ctx, w: bool) {
    let (sentinel, mask) = if w { (127i64, 63i64) } else { (63, 31) };
    f.pop_to(A);
    f.asm.bsr(w, A, A);
    f.asm.mov_r64_imm64(B, sentinel);
    f.asm.cmov(w, Cond::E, A, B);
    f.asm.mov_r64_imm64(B, mask);
    f.asm.xor(w, A, B);
    f.push_from_ty(A, if w { ValType::I64 } else { ValType::I32 });
}

/// `ctz`: `bsf` is already the answer for a non-zero input; zero takes the
/// full width.
fn ctz(f: &mut Ctx, w: bool) {
    let width = if w { 64i64 } else { 32 };
    f.pop_to(A);
    f.asm.bsf(w, A, A);
    f.asm.mov_r64_imm64(B, width);
    f.asm.cmov(w, Cond::E, A, B);
    f.push_from_ty(A, if w { ValType::I64 } else { ValType::I32 });
}
