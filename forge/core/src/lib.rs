//! forge — WASM to x86-64, single pass, at `install` time.
//!
//! Decoding and validation come from wasmparser, which the kernel already
//! carries through wasmi (`no_std`, `validate`, no serde, no hashbrown).
//! What is ours is the code generation.
//!
//! `FEATURES` below is the contract between validator and code generator:
//! the validator rejects anything the generator cannot emit, so a module
//! that reaches codegen is by construction inside the 164 opcodes counted
//! over every module in `release/modules/`. Widening it is a decision, not
//! an accident — see docs/plan/WASM_SPEED_2026_08.md.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(feature = "census")]
use alloc::collections::BTreeSet;

pub mod codegen;
pub mod x64;

/// Byte offsets inside the instance context, which generated code reaches
/// through the pinned register. Fixed here so the generator and whatever
/// builds the context cannot drift apart — a wrong offset here is a wild
/// pointer, not a compile error.
pub mod vmctx {
    /// Base of linear memory. Pinned in its own register later; for now the
    /// generator loads it per access.
    pub const MEM_BASE: i32 = 0;
    /// Current size of linear memory in bytes.
    pub const MEM_SIZE: i32 = 8;
    /// Base of the globals array, eight bytes per global.
    pub const GLOBALS: i32 = 16;
    /// Base of the function table: one code pointer per slot.
    pub const TABLE: i32 = 24;
    pub const TABLE_LEN: i32 = 32;
    /// Fuel remaining. Moves into a pinned register once metering lands.
    pub const FUEL: i32 = 40;
    /// Array of host function pointers, one per import.
    pub const HOST_FNS: i32 = 48;
    /// Canonical signature id per table slot, four bytes each. Kept beside the
    /// table rather than inside it so both arrays use a natural x86 scale —
    /// eight for the pointers, four for the ids.
    pub const TABLE_SIGS: i32 = 56;
    /// Largest the memory may become, in pages. `memory.grow` refuses beyond
    /// it and answers -1, which is a RESULT in wasm and not a trap.
    pub const MEM_MAX_PAGES: i32 = 64;
    /// The one runtime routine generated code has to call: growing memory
    /// needs a mapping changed, which no instruction can do.
    pub const BUILTIN_GROW: i32 = 72;

    /// Where a trap goes. The entry trampoline records the stack it was called
    /// on and the address to resume at; a trap anywhere inside the module
    /// restores those two and jumps, which unwinds any depth of wasm frames in
    /// four instructions and without touching the native stack discipline.
    ///
    /// This is what a fault handler needs as well: catching #PF from a guard
    /// page or #DE from a divide means pointing the interrupted context at the
    /// module's trap routine, and everything below happens by itself.
    pub const TRAP_RSP: i32 = 80;
    pub const TRAP_RBP: i32 = 88;
    pub const TRAP_RESUME: i32 = 96;
    /// What went wrong; `trap::NONE` after a clean return.
    pub const TRAP_CODE: i32 = 104;

    pub const SIZE: usize = 112;
    /// Every global occupies eight bytes regardless of type, so the index is
    /// a plain shift.
    pub const GLOBAL_STRIDE: i32 = 8;
}

/// Why a module stopped. A trap is a RESULT in wasm — the module is finished,
/// but nothing else is wrong — so it has to be reportable rather than fatal.
pub mod trap {
    pub const NONE: u32 = 0;
    pub const UNREACHABLE: u32 = 1;
    pub const MEMORY_OUT_OF_BOUNDS: u32 = 2;
    pub const TABLE_OUT_OF_BOUNDS: u32 = 3;
    pub const BAD_SIGNATURE: u32 = 4;
    /// Division by zero, and `INT_MIN / -1`. The processor raises the same
    /// fault for both and does not say which, so neither do we.
    pub const DIVIDE_ERROR: u32 = 5;
    pub const OUT_OF_FUEL: u32 = 6;
    /// A call reached a function the generator refused to translate.
    pub const UNCOMPILED: u32 = 7;

    pub fn name(code: u32) -> &'static str {
        match code {
            NONE => "kein Trap",
            UNREACHABLE => "unreachable",
            MEMORY_OUT_OF_BOUNDS => "Speicherzugriff daneben",
            TABLE_OUT_OF_BOUNDS => "Tabellenindex daneben",
            BAD_SIGNATURE => "falsche Signatur beim indirekten Aufruf",
            DIVIDE_ERROR => "Division durch null oder Ueberlauf",
            OUT_OF_FUEL => "Fuel aufgebraucht",
            UNCOMPILED => "Funktion nicht uebersetzt",
            _ => "unbekannt",
        }
    }
}

use wasmparser::{
    FuncValidatorAllocations, FunctionBody, Operator, Parser, Payload, ValType, ValidPayload,
    Validator, WasmFeatures,
};

/// Exactly what our modules use, and nothing more. No SIMD, no threads, no
/// reference types, no exceptions, no multi-value.
///
/// `CALL_INDIRECT_OVERLONG` is an ENCODING allowance, not a proposal: LLVM
/// writes `call_indirect`'s table immediate as an overlong LEB, which was
/// illegal before reference types. Without it 13 of our 21 modules — every
/// one that has a `call_indirect` — are rejected at the first one. It admits
/// no new opcode and no new semantics.
pub const fn features() -> WasmFeatures {
    WasmFeatures::WASM1
        .union(WasmFeatures::BULK_MEMORY)
        .union(WasmFeatures::SIGN_EXTENSION)
        .union(WasmFeatures::SATURATING_FLOAT_TO_INT)
        .union(WasmFeatures::CALL_INDIRECT_OVERLONG)
}

/// Opcode names, straight from wasmparser's own operator list, so neither the
/// census nor the generator's refusal message can drift from what the reader
/// accepts.
pub(crate) mod names {
    macro_rules! name_of {
        ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
            pub fn op_name(op: &wasmparser::Operator) -> &'static str {
                match op {
                    $( wasmparser::Operator::$op $( { $($arg: _),* } )? => stringify!($op), )*
                    // `Operator` is non_exhaustive; the validator has already
                    // rejected anything outside `features()` by this point.
                    _ => "?",
                }
            }
        };
    }
    wasmparser::for_each_operator!(name_of);
}

/// Opcodes the generator has a case for. Ordering advice only — never the
/// progress number.
///
/// A list cannot say the truth here: `End` is emitted for a function's final
/// end but not for a block's, so "End is implemented" is true and misleading
/// at once. The real measure is `compile()` itself — count the functions that
/// came back `Done`. That is what `--roadmap` reports.
pub const IMPLEMENTED: &[&str] = &[
    "Nop", "End", "Return", "Drop", "Unreachable",
    "Block", "Loop", "If", "Else", "Br", "BrIf",
    "LocalGet", "LocalSet", "LocalTee", "GlobalGet", "GlobalSet", "I32Const",
    "I32Add", "I32Sub", "I32Mul", "I32And", "I32Or", "I32Xor",
    "I32Eq", "I32Ne", "I32LtS", "I32LtU", "I32GtS", "I32GtU",
    "I32LeS", "I32LeU", "I32GeS", "I32GeU", "I32Eqz",
    "I32Load", "I32Load8U", "I32Load8S", "I32Load16U", "I32Load16S",
    "I32Store", "I32Store8", "I32Store16",
    "I32Shl", "I32ShrU", "I32ShrS", "I32Rotl", "I32Rotr",
    "Call", "CallIndirect", "BrTable", "Select",
    "I32Clz", "I32Ctz", "I32Popcnt", "I32Extend8S", "I32Extend16S",
    "I64Const", "I64Add", "I64Sub", "I64Mul", "I64And", "I64Or", "I64Xor",
    "I64Shl", "I64ShrU", "I64ShrS", "I64Rotl", "I64Rotr",
    "I64Eq", "I64Ne", "I64LtS", "I64LtU", "I64GtS", "I64GtU",
    "I64LeS", "I64LeU", "I64GeS", "I64GeU", "I64Eqz", "I64Clz", "I64Ctz",
    "I64Load", "I64Load8U", "I64Load8S", "I64Load16U", "I64Load16S",
    "I64Load32U", "I64Load32S",
    "I64Store", "I64Store8", "I64Store16", "I64Store32",
    "I32WrapI64", "I64ExtendI32U", "I64ExtendI32S",
    "I64Extend8S", "I64Extend16S", "I64Extend32S",
    "MemoryCopy", "MemoryFill",
    "I32DivS", "I32DivU", "I32RemS", "I32RemU",
    "I64DivS", "I64DivU", "I64RemS", "I64RemU",
    "MemorySize",
    "F32Const", "F64Const", "F32Load", "F64Load", "F32Store", "F64Store",
    "F32Add", "F32Sub", "F32Mul", "F32Div", "F64Add", "F64Sub", "F64Mul", "F64Div",
    "F32Min", "F32Max", "F64Min", "F64Max",
    "F32Sqrt", "F64Sqrt", "F32Floor", "F64Floor", "F32Ceil", "F64Ceil",
    "F32Trunc", "F64Trunc", "F32Nearest", "F64Nearest",
    "F32Abs", "F64Abs", "F32Neg", "F64Neg", "F32Copysign", "F64Copysign",
    "F32Eq", "F32Ne", "F32Lt", "F32Gt", "F32Le", "F32Ge",
    "F64Eq", "F64Ne", "F64Lt", "F64Gt", "F64Le", "F64Ge",
    "F32DemoteF64", "F64PromoteF32",
    "F32ConvertI32S", "F32ConvertI32U", "F32ConvertI64S", "F32ConvertI64U",
    "F64ConvertI32S", "F64ConvertI32U", "F64ConvertI64S", "F64ConvertI64U",
    "I32TruncSatF32S", "I32TruncSatF32U", "I32TruncSatF64S", "I32TruncSatF64U",
    "I64TruncSatF32S", "I64TruncSatF64S",
    "I32ReinterpretF32", "F32ReinterpretI32",
    "I64ReinterpretF64", "F64ReinterpretI64",
    "MemoryGrow",
];

#[derive(Debug)]
pub enum Error {
    /// The module is malformed, or uses something outside `features()`.
    Reject(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Reject(m) => write!(f, "{m}"),
        }
    }
}

/// What one function costs the code generator: frame size, fuel granularity,
/// and whether it touches the parts that need a runtime (memory, calls).
#[derive(Default, Debug, Clone)]
pub struct FuncPlan {
    /// Index in the module's function index space — imports included.
    pub func_index: u32,
    /// Declared locals beyond the parameters, already summed per type.
    pub locals: u32,
    /// Deepest the operand stack ever gets — the frame's spill area.
    pub max_stack: u32,
    pub instrs: u32,
    /// Points where a fuel counter held in a register must reach memory:
    /// every control-flow edge and every call. One `sub` per span between
    /// them, so `instrs / flushes` is the accounting granularity.
    pub flushes: u32,
    pub calls: u32,
    pub indirect: u32,
    pub mem_ops: u32,
    /// Memory immediates above `i32::MAX`. `disp32` is signed, so these need
    /// the offset folded into the address register — a path that cannot be
    /// exercised in bounds, so it is worth knowing whether it ever occurs.
    pub big_offsets: u32,
    /// Distinct opcodes this function uses. A function is compilable exactly
    /// when this set is a subset of what the generator emits, so this is the
    /// progress meter: how much of a real module we can already translate.
    #[cfg(feature = "census")]
    pub ops: BTreeSet<&'static str>,
}

/// The module shape the code generator needs before it emits a byte.
#[derive(Default, Debug)]
pub struct ModulePlan {
    /// (params, results) per type index.
    pub types: Vec<(Vec<ValType>, Vec<ValType>)>,
    /// `module::name` per imported function, in index order — these occupy
    /// the low function indices, ahead of the defined ones.
    pub imported_funcs: Vec<(String, String)>,
    /// Type index per *defined* function, in index order.
    pub funcs: Vec<u32>,
    /// Type index per function index across the WHOLE space — imports first,
    /// then defined. What a `call` needs to know the callee's shape.
    pub func_type_of: Vec<u32>,
    pub memory: Option<(u64, Option<u64>)>,
    pub table: Option<(u32, Option<u32>)>,
    /// Type and mutability per global, imported ones first — they hold the
    /// low indices, exactly like functions.
    pub global_types: Vec<(ValType, bool)>,
    /// Constant initialiser per defined global, widened to i64. `None` where
    /// the initialiser is not a plain constant.
    pub global_init: Vec<Option<i64>>,
    pub exports: Vec<(String, u32)>,
    pub start: Option<u32>,
    pub elem_segments: u32,
    pub data_segments: u32,
    /// Active data segments as (offset, bytes) — what an instance must copy
    /// into linear memory before the first instruction runs.
    pub data_init: Vec<(u32, Vec<u8>)>,
    /// Active element segments as (offset, function indices) — what fills the
    /// table.
    pub elem_init: Vec<(u32, Vec<u32>)>,
    /// Canonical signature id per type index. wasm compares function types
    /// STRUCTURALLY, so two different type indices with the same shape must
    /// pass the same `call_indirect` check — comparing raw type indices would
    /// reject calls the spec allows.
    pub sig_id: Vec<u32>,
    pub bodies: Vec<FuncPlan>,
}

impl ModulePlan {
    /// Function index of the first *defined* function; everything below is an
    /// import and is called through the host, not with a near call.
    pub fn first_defined(&self) -> u32 {
        self.imported_funcs.len() as u32
    }

    pub fn total_instrs(&self) -> u64 {
        self.bodies.iter().map(|b| b.instrs as u64).sum()
    }

    pub fn total_flushes(&self) -> u64 {
        self.bodies.iter().map(|b| b.flushes as u64).sum()
    }

    pub fn max_stack(&self) -> u32 {
        self.bodies.iter().map(|b| b.max_stack).max().unwrap_or(0)
    }
}

/// A global's initialiser, when it is a single constant. Anything else —
/// `global.get` of an imported global, say — comes back `None` rather than a
/// guess.
fn const_init(expr: &wasmparser::ConstExpr<'_>) -> Option<i64> {
    let mut r = expr.get_operators_reader();
    let first = r.read().ok()?;
    let v = match first {
        Operator::I32Const { value } => value as i64,
        Operator::I64Const { value } => value,
        Operator::F32Const { value } => value.bits() as i64,
        Operator::F64Const { value } => value.bits() as i64,
        _ => return None,
    };
    match r.read().ok()? {
        Operator::End => Some(v),
        _ => None,
    }
}

/// Give structurally identical function types the same id, so a
/// `call_indirect` check compares shapes and not the order the types happened
/// to be written in.
fn canonical_sig_ids(types: &[(Vec<ValType>, Vec<ValType>)]) -> Vec<u32> {
    let mut seen: Vec<&(Vec<ValType>, Vec<ValType>)> = Vec::new();
    let mut out = Vec::with_capacity(types.len());
    for t in types {
        match seen.iter().position(|s| *s == t) {
            Some(i) => out.push(i as u32),
            None => {
                seen.push(t);
                out.push(seen.len() as u32 - 1);
            }
        }
    }
    out
}

fn reject(e: impl core::fmt::Display) -> Error {
    Error::Reject(e.to_string())
}

/// A module after code generation. `funcs` runs parallel to `plan.bodies`.
pub struct CompiledModule {
    pub plan: ModulePlan,
    pub funcs: Vec<codegen::Outcome>,
    /// Every compiled function laid out end to end, with a trap stub last.
    /// Calls are near-relative, so the functions of one module have to share
    /// one object — that is what makes a call a single instruction instead of
    /// a load and an indirect jump.
    pub code: Vec<u8>,
    /// Offset into `code` per DEFINED function; `None` where generation
    /// refused. A call to one of those is aimed at the trap stub, so a
    /// half-translated module cannot quietly run into the wrong place.
    pub offsets: Vec<Option<usize>>,
    /// Where the entry trampoline sits. Native code enters a module here, not
    /// at a function directly.
    pub entry_offset: usize,
    /// Where the trap routine sits.
    pub trap_routine: usize,
    /// Where a page fault and a divide fault should be sent. Each is two
    /// instructions that name the reason and fall into the trap routine.
    ///
    /// Why per reason rather than one entry plus a register: a fault handler
    /// can rewrite the interrupted instruction pointer with almost nothing —
    /// the saved value sits in the interrupt frame. Rewriting a general
    /// register means unpicking however the handler saved them. Two extra
    /// stubs cost ten bytes and remove that problem entirely.
    pub pf_entry: usize,
    pub de_entry: usize,
    /// Where the trap stub sits. A table slot whose function did not compile
    /// points here, so a half-translated module traps instead of jumping into
    /// whatever happens to follow.
    pub trap_offset: usize,
}

impl CompiledModule {
    /// Offset of a function by its index in the whole space. Imports have no
    /// code here.
    pub fn offset_of(&self, function_index: u32) -> Option<usize> {
        let n = self.plan.imported_funcs.len() as u32;
        if function_index < n {
            return None;
        }
        *self.offsets.get((function_index - n) as usize)?
    }
}

/// Validate and generate. A function that meets an opcode the generator does
/// not emit comes back as `Unsupported(name)` — never as wrong code.
pub fn compile(wasm: &[u8]) -> Result<CompiledModule, Error> {
    let mut lk = Linker::new(wasm.len());
    let (plan, funcs) = walk(wasm, Some(&mut lk))?;
    let l = lk.finish(&plan, &funcs);
    Ok(CompiledModule {
        plan,
        funcs,
        code: l.code,
        offsets: l.offsets,
        trap_offset: l.trap_offset,
        entry_offset: l.entry_offset,
        trap_routine: l.trap_routine,
        pf_entry: l.pf_entry,
        de_entry: l.de_entry,
    })
}

/// Place every compiled function in one buffer and resolve the `call rel32`
/// displacements that generation had to leave blank.
struct Linked {
    code: Vec<u8>,
    offsets: Vec<Option<usize>>,
    trap_offset: usize,
    entry_offset: usize,
    trap_routine: usize,
    pf_entry: usize,
    de_entry: usize,
}

/// Places functions as they come out of the generator, instead of collecting
/// them all and concatenating at the end.
///
/// The difference is not tidiness. Holding every function's buffer until the
/// end means thousands of live blocks in a heap whose free list is walked on
/// every allocation — and on the device that turned a translation that scales
/// linearly with output size into one that scales eight times worse. Taking
/// each function's bytes immediately, and letting its buffer go, keeps the
/// number of live blocks roughly flat no matter how large the module is.
struct Linker {
    code: Vec<u8>,
    offsets: Vec<Option<usize>>,
    /// (offset of the rel32, target function index) — absolute in `code`.
    relocs: Vec<(usize, u32)>,
    trap_relocs: Vec<usize>,
    entry_offset: usize,
    trap_routine: usize,
    pf_entry: usize,
    de_entry: usize,
}

impl Linker {
    /// `wasm_len` only sizes the first allocation. Generated code runs between
    /// one and a half and two and a half times the whole module's bytes, so
    /// twice is a guess that usually holds and never costs more than one
    /// growth if it does not.
    fn new(wasm_len: usize) -> Linker {
        let mut code: Vec<u8> = Vec::with_capacity(wasm_len * 2 + 4096);

        let entry_offset = 0usize;
        let mut a = x64::Asm::new();
        codegen::emit_entry(&mut a);
        code.extend_from_slice(&a.finish().unwrap_or_default());

        let trap_routine = code.len();
        let mut a = x64::Asm::new();
        codegen::emit_trap_routine(&mut a);
        code.extend_from_slice(&a.finish().unwrap_or_default());

        // One entry per processor fault the generator relies on. A handler
        // only has to point the interrupted instruction pointer at the right
        // one; the entry names the reason itself.
        let mut fault_entry = |code: &mut Vec<u8>, reason: u32| -> usize {
            let at = code.len();
            let mut a = x64::Asm::new();
            a.mov_r32_imm32(x64::Reg::Rax, reason as i32);
            let hole = a.jmp_rel32_blank();
            let bytes = a.finish().unwrap_or_default();
            code.extend_from_slice(&bytes);
            let here = at + hole;
            let rel = trap_routine as i64 - (here as i64 + 4);
            code[here..here + 4].copy_from_slice(&(rel as i32).to_le_bytes());
            at
        };
        let pf_entry = fault_entry(&mut code, trap::MEMORY_OUT_OF_BOUNDS);
        let de_entry = fault_entry(&mut code, trap::DIVIDE_ERROR);

        Linker {
            code,
            offsets: Vec::new(),
            relocs: Vec::new(),
            trap_relocs: Vec::new(),
            entry_offset,
            trap_routine,
            pf_entry,
            de_entry,
        }
    }

    /// Take one function's bytes and let go of its buffer.
    fn push(&mut self, out: &mut codegen::Outcome) {
        let codegen::Outcome::Done(cf) = out else {
            self.offsets.push(None);
            return;
        };
        let base = self.code.len();
        self.offsets.push(Some(base));
        self.code.extend_from_slice(&cf.code);
        for r in &cf.relocs {
            self.relocs.push((base + r.at, r.target));
        }
        for at in &cf.trap_relocs {
            self.trap_relocs.push(base + at);
        }
        // Handed over: the module owns these bytes now, and one fewer live
        // block is one fewer stop on every future walk of the free list.
        cf.code = Vec::new();
        cf.relocs = Vec::new();
        cf.trap_relocs = Vec::new();
    }

    fn finish(mut self, plan: &ModulePlan, _funcs: &[codegen::Outcome]) -> Linked {
        // Where a call to a function the generator refused lands. It reports
        // itself like any other trap instead of faulting.
        let trap = self.code.len();
        {
            let mut a = x64::Asm::new();
            a.mov_r32_imm32(x64::Reg::Rax, trap::UNCOMPILED as i32);
            let hole = a.jmp_rel32_blank();
            let bytes = a.finish().unwrap_or_default();
            let here = trap + hole;
            self.code.extend_from_slice(&bytes);
            let rel = self.trap_routine as i64 - (here as i64 + 4);
            self.code[here..here + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }

        for at in &self.trap_relocs {
            let rel = self.trap_routine as i64 - (*at as i64 + 4);
            self.code[*at..*at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }

        let n_imported = plan.imported_funcs.len() as u32;
        for (at, target) in &self.relocs {
            let dst = target
                .checked_sub(n_imported)
                .and_then(|d| self.offsets.get(d as usize).copied().flatten())
                .unwrap_or(trap);
            let rel = dst as i64 - (*at as i64 + 4);
            self.code[*at..*at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }

        Linked {
            code: self.code,
            offsets: self.offsets,
            trap_offset: trap,
            entry_offset: self.entry_offset,
            trap_routine: self.trap_routine,
            pf_entry: self.pf_entry,
            de_entry: self.de_entry,
        }
    }
}

/// The loop the code generator will live in: read an operator, hand it to the
/// validator, then emit. Here it only measures — but the shape is final.
/// Validate a module against `features()` and collect what the code generator
/// needs. One pass: the validator's type stack is the same one the register
/// allocator will ride on, so nothing is walked twice.
pub fn plan(wasm: &[u8]) -> Result<ModulePlan, Error> {
    Ok(walk(wasm, None)?.0)
}

fn walk(
    wasm: &[u8],
    mut lk: Option<&mut Linker>,
) -> Result<(ModulePlan, Vec<codegen::Outcome>), Error> {
    let generate = lk.is_some();
    let mut out: Vec<codegen::Outcome> = Vec::new();
    let mut plan = ModulePlan::default();
    // One buffer for the operator list, reused for every function. A fresh
    // one per function is ten thousand allocations that say nothing.
    let mut ops_buf: Vec<Operator> = Vec::new();
    let mut validator = Validator::new_with_features(features());
    let mut allocs = FuncValidatorAllocations::default();

    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.map_err(reject)?;
        let valid = validator.payload(&payload).map_err(reject)?;

        match &payload {
            Payload::TypeSection(r) => {
                for group in r.clone().into_iter() {
                    for ty in group.map_err(reject)?.into_types() {
                        // Never `unwrap_func()` — it panics, and this input
                        // is not trusted.
                        let wasmparser::CompositeInnerType::Func(ft) = &ty.composite_type.inner
                        else {
                            return Err(Error::Reject("non-function type".to_string()));
                        };
                        plan.types
                            .push((ft.params().to_vec(), ft.results().to_vec()));
                    }
                }
            }
            Payload::ImportSection(r) => {
                for imp in r.clone() {
                    let imp = imp.map_err(reject)?;
                    match imp.ty {
                        wasmparser::TypeRef::Func(t) => {
                            plan.imported_funcs
                                .push((imp.module.to_string(), imp.name.to_string()));
                            plan.func_type_of.push(t);
                        }
                        wasmparser::TypeRef::Global(g) => {
                            plan.global_types.push((g.content_type, g.mutable))
                        }
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for ti in r.clone() {
                    let ti = ti.map_err(reject)?;
                    plan.funcs.push(ti);
                    plan.func_type_of.push(ti);
                }
            }
            Payload::MemorySection(r) => {
                for m in r.clone() {
                    let m = m.map_err(reject)?;
                    plan.memory = Some((m.initial, m.maximum));
                }
            }
            Payload::TableSection(r) => {
                for t in r.clone() {
                    let t = t.map_err(reject)?;
                    plan.table = Some((t.ty.initial as u32, t.ty.maximum.map(|v| v as u32)));
                }
            }
            Payload::GlobalSection(r) => {
                for g in r.clone() {
                    let g = g.map_err(reject)?;
                    plan.global_types.push((g.ty.content_type, g.ty.mutable));
                    plan.global_init.push(const_init(&g.init_expr));
                }
            }
            Payload::ExportSection(r) => {
                for e in r.clone() {
                    let e = e.map_err(reject)?;
                    if let wasmparser::ExternalKind::Func = e.kind {
                        plan.exports.push((e.name.to_string(), e.index));
                    }
                }
            }
            Payload::StartSection { func, .. } => plan.start = Some(*func),
            Payload::ElementSection(r) => {
                plan.elem_segments += r.count();
                for e in r.clone() {
                    let e = e.map_err(reject)?;
                    let wasmparser::ElementKind::Active {
                        table_index: None | Some(0),
                        offset_expr,
                    } = e.kind
                    else {
                        continue;
                    };
                    let Some(off) = const_init(&offset_expr) else {
                        continue;
                    };
                    if let wasmparser::ElementItems::Functions(fs) = e.items {
                        let mut idxs = Vec::new();
                        for f in fs {
                            idxs.push(f.map_err(reject)?);
                        }
                        plan.elem_init.push((off as u32, idxs));
                    }
                }
            }
            Payload::DataSection(r) => {
                plan.data_segments += r.count();
                for d in r.clone() {
                    let d = d.map_err(reject)?;
                    if let wasmparser::DataKind::Active {
                        memory_index: 0,
                        offset_expr,
                    } = d.kind
                    {
                        if let Some(off) = const_init(&offset_expr) {
                            plan.data_init.push((off as u32, d.data.to_vec()));
                        }
                    }
                }
            }
            _ => {}
        }

        if plan.sig_id.len() != plan.types.len() {
            plan.sig_id = canonical_sig_ids(&plan.types);
        }

        if let ValidPayload::Func(to_validate, body) = valid {
            let mut fv = to_validate.into_validator(core::mem::take(&mut allocs));
            ops_buf.clear();
            let fp = walk_body(&mut fv, &body, generate, &mut ops_buf)?;
            allocs = fv.into_allocations();

            if let Some(l) = lk.as_deref_mut() {
                let mut o = generate_one(&plan, &fp, &body, &ops_buf);
                // Straight into the module buffer, and the function's own
                // buffer goes back to the heap right away.
                l.push(&mut o);
                out.push(o);
            }
            plan.bodies.push(fp);
        }
    }

    Ok((plan, out))
}

/// Hand one validated body to the generator, with the signature and the local
/// declarations it needs. Anything missing is a refusal, not a guess.
fn generate_one(
    plan: &ModulePlan,
    fp: &FuncPlan,
    body: &FunctionBody<'_>,
    ops: &[Operator<'_>],
) -> codegen::Outcome {
    let ordinal = fp.func_index as usize - plan.imported_funcs.len();
    let Some(&ti) = plan.funcs.get(ordinal) else {
        return codegen::Outcome::Unsupported("no-type");
    };
    let Some((params, results)) = plan.types.get(ti as usize) else {
        return codegen::Outcome::Unsupported("no-type");
    };
    let Ok(lr) = body.get_locals_reader() else {
        return codegen::Outcome::Unsupported("locals");
    };
    let mut decls = Vec::new();
    for d in lr {
        match d {
            Ok(v) => decls.push(v),
            Err(_) => return codegen::Outcome::Unsupported("locals"),
        }
    }
    let m = codegen::ModuleCtx {
        types: &plan.types,
        func_type_of: &plan.func_type_of,
        n_imported: plan.imported_funcs.len() as u32,
        sig_id: &plan.sig_id,
        globals: &plan.global_types,
    };
    codegen::compile_func(params, results, &decls, ops, &m)
}

fn walk_body<'a, T: wasmparser::WasmModuleResources>(
    fv: &mut wasmparser::FuncValidator<T>,
    body: &FunctionBody<'a>,
    keep_ops: bool,
    kept: &mut Vec<Operator<'a>>,
) -> Result<FuncPlan, Error> {
    let mut p = FuncPlan {
        func_index: fv.index(),
        ..Default::default()
    };

    let mut reader = body.get_binary_reader();
    fv.read_locals(&mut reader).map_err(reject)?;
    reader.set_features(*fv.features());
    p.locals = fv.len_locals();

    let mut ops = wasmparser::OperatorsReader::new(reader);
    while !ops.eof() {
        let pos = ops.original_position();
        let op: Operator = ops.read().map_err(reject)?;
        fv.op(pos, &op).map_err(reject)?;

        p.instrs += 1;
        #[cfg(feature = "census")]
        p.ops.insert(names::op_name(&op));
        if keep_ops {
            kept.push(op.clone());
        }
        let h = fv.operand_stack_height();
        if h > p.max_stack {
            p.max_stack = h;
        }

        match op {
            // Every edge out of a basic block, plus every call: the points
            // where a register-held fuel counter has to reach memory.
            Operator::Unreachable
            | Operator::Loop { .. }
            | Operator::If { .. }
            | Operator::Else
            | Operator::Br { .. }
            | Operator::BrIf { .. }
            | Operator::BrTable { .. }
            | Operator::End
            | Operator::Return => p.flushes += 1,
            Operator::Call { .. } => {
                p.flushes += 1;
                p.calls += 1;
            }
            Operator::CallIndirect { .. } => {
                p.flushes += 1;
                p.indirect += 1;
            }
            Operator::MemoryCopy { .. } | Operator::MemoryFill { .. } => p.mem_ops += 1,
            ref other => {
                if let Some(m) = memarg_of(other) {
                    p.mem_ops += 1;
                    if m.offset > i32::MAX as u64 {
                        p.big_offsets += 1;
                    }
                }
            }
        }
    }

    ops.finish().map_err(reject)?;
    Ok(p)
}

/// Accesses to linear memory — the ones that ride the guard-page reservation
/// and therefore need no bounds-check instruction of their own.
fn memarg_of<'a>(op: &'a Operator<'_>) -> Option<&'a wasmparser::MemArg> {
    use Operator::*;
    match op {
        I32Load { memarg }
        | I64Load { memarg }
        | F32Load { memarg }
        | F64Load { memarg }
        | I32Load8S { memarg }
        | I32Load8U { memarg }
        | I32Load16S { memarg }
        | I32Load16U { memarg }
        | I64Load8S { memarg }
        | I64Load8U { memarg }
        | I64Load16S { memarg }
        | I64Load16U { memarg }
        | I64Load32S { memarg }
        | I64Load32U { memarg }
        | I32Store { memarg }
        | I64Store { memarg }
        | F32Store { memarg }
        | F64Store { memarg }
        | I32Store8 { memarg }
        | I32Store16 { memarg }
        | I64Store8 { memarg }
        | I64Store16 { memarg }
        | I64Store32 { memarg } => Some(memarg),
        _ => None,
    }
}

#[allow(dead_code)]
fn is_load_or_store(op: &Operator<'_>) -> bool {
    use Operator::*;
    matches!(
        op,
        I32Load { .. }
            | I64Load { .. }
            | F32Load { .. }
            | F64Load { .. }
            | I32Load8S { .. }
            | I32Load8U { .. }
            | I32Load16S { .. }
            | I32Load16U { .. }
            | I64Load8S { .. }
            | I64Load8U { .. }
            | I64Load16S { .. }
            | I64Load16U { .. }
            | I64Load32S { .. }
            | I64Load32U { .. }
            | I32Store { .. }
            | I64Store { .. }
            | F32Store { .. }
            | F64Store { .. }
            | I32Store8 { .. }
            | I32Store16 { .. }
            | I64Store8 { .. }
            | I64Store16 { .. }
            | I64Store32 { .. }
    )
}
