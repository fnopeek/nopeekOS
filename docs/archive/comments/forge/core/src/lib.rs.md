# `forge/core/src/lib.rs` @ 5e0102684

## L1-11 · `#![no_std]`

```
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
```

## L25-28 · `pub mod vmctx {`

```
/// Byte offsets inside the instance context, which generated code reaches
/// through the pinned register. Fixed here so the generator and whatever
/// builds the context cannot drift apart — a wrong offset here is a wild
/// pointer, not a compile error.
```

## L30-31 · `pub const MEM_BASE: i32 = 0;`

```
/// Base of linear memory. Pinned in its own register later; for now the
/// generator loads it per access.
```

## L33 · `pub const MEM_SIZE: i32 = 8;`

```
/// Current size of linear memory in bytes.
```

## L35 · `pub const GLOBALS: i32 = 16;`

```
/// Base of the globals array, eight bytes per global.
```

## L37 · `pub const TABLE: i32 = 24;`

```
/// Base of the function table: one code pointer per slot.
```

## L40 · `pub const FUEL: i32 = 40;`

```
/// Fuel remaining. Moves into a pinned register once metering lands.
```

## L42 · `pub const HOST_FNS: i32 = 48;`

```
/// Array of host function pointers, one per import.
```

## L44-46 · `pub const TABLE_SIGS: i32 = 56;`

```
/// Canonical signature id per table slot, four bytes each. Kept beside the
/// table rather than inside it so both arrays use a natural x86 scale —
/// eight for the pointers, four for the ids.
```

## L48-49 · `pub const MEM_MAX_PAGES: i32 = 64;`

```
/// Largest the memory may become, in pages. `memory.grow` refuses beyond
/// it and answers -1, which is a RESULT in wasm and not a trap.
```

## L51-52 · `pub const BUILTIN_GROW: i32 = 72;`

```
/// The one runtime routine generated code has to call: growing memory
/// needs a mapping changed, which no instruction can do.
```

## L55-62 · `pub const TRAP_RSP: i32 = 80;`

```
/// Where a trap goes. The entry trampoline records the stack it was called
/// on and the address to resume at; a trap anywhere inside the module
/// restores those two and jumps, which unwinds any depth of wasm frames in
/// four instructions and without touching the native stack discipline.
///
/// This is what a fault handler needs as well: catching #PF from a guard
/// page or #DE from a divide means pointing the interrupted context at the
/// module's trap routine, and everything below happens by itself.
```

## L66 · `pub const TRAP_CODE: i32 = 104;`

```
/// What went wrong; `trap::NONE` after a clean return.
```

## L69-72 · `pub const HOST_CTX: i32 = 112;`

```
/// The embedder's own state, opaque here. Generated code never reads it;
/// a host function does, to find the caller it belongs to. Without it a
/// host function would need a static, and a static cannot serve two
/// modules on two cores.
```

## L76-77 · `pub const GLOBAL_STRIDE: i32 = 8;`

```
/// Every global occupies eight bytes regardless of type, so the index is
/// a plain shift.
```

## L81-82 · `pub mod trap {`

```
/// Why a module stopped. A trap is a RESULT in wasm — the module is finished,
/// but nothing else is wrong — so it has to be reportable rather than fatal.
```

## L89-90 · `pub const DIVIDE_ERROR: u32 = 5;`

```
/// Division by zero, and `INT_MIN / -1`. The processor raises the same
/// fault for both and does not say which, so neither do we.
```

## L93 · `pub const UNCOMPILED: u32 = 7;`

```
/// A call reached a function the generator refused to translate.
```

## L95-98 · `pub const EXIT: u32 = 8;`

```
/// A host function ended the run instead of returning. wasi programs leave
/// this way — through `proc_exit`, a clean finish included. The status
/// itself does NOT travel here; it belongs to the embedder's state, which
/// the host function already holds.
```

## L122-129 · `pub const fn features() -> WasmFeatures {`

```
/// Exactly what our modules use, and nothing more. No SIMD, no threads, no
/// reference types, no exceptions, no multi-value.
///
/// `CALL_INDIRECT_OVERLONG` is an ENCODING allowance, not a proposal: LLVM
/// writes `call_indirect`'s table immediate as an overlong LEB, which was
/// illegal before reference types. Without it 13 of our 21 modules — every
/// one that has a `call_indirect` — are rejected at the first one. It admits
/// no new opcode and no new semantics.
```

## L138-140 · `pub(crate) mod names {`

```
/// Opcode names, straight from wasmparser's own operator list, so neither the
/// census nor the generator's refusal message can drift from what the reader
/// accepts.
```

## L147-148 · `_ => "?",`

```
// `Operator` is non_exhaustive; the validator has already
// rejected anything outside `features()` by this point.
```

## L157-163 · `pub const IMPLEMENTED: &[&str] = &[`

```
/// Opcodes the generator has a case for. Ordering advice only — never the
/// progress number.
///
/// A list cannot say the truth here: `End` is emitted for a function's final
/// end but not for a block's, so "End is implemented" is true and misleading
/// at once. The real measure is `compile()` itself — count the functions that
/// came back `Done`. That is what `--roadmap` reports.
```

## L209 · `Reject(String),`

```
/// The module is malformed, or uses something outside `features()`.
```

## L221-222 · `#[derive(Default, Debug, Clone)]`

```
/// What one function costs the code generator: frame size, fuel granularity,
/// and whether it touches the parts that need a runtime (memory, calls).
```

## L225 · `pub func_index: u32,`

```
/// Index in the module's function index space — imports included.
```

## L227 · `pub locals: u32,`

```
/// Declared locals beyond the parameters, already summed per type.
```

## L229 · `pub max_stack: u32,`

```
/// Deepest the operand stack ever gets — the frame's spill area.
```

## L232-234 · `pub flushes: u32,`

```
/// Points where a fuel counter held in a register must reach memory:
/// every control-flow edge and every call. One `sub` per span between
/// them, so `instrs / flushes` is the accounting granularity.
```

## L239-241 · `pub big_offsets: u32,`

```
/// Memory immediates above `i32::MAX`. `disp32` is signed, so these need
/// the offset folded into the address register — a path that cannot be
/// exercised in bounds, so it is worth knowing whether it ever occurs.
```

## L243-245 · `#[cfg(feature = "census")]`

```
/// Distinct opcodes this function uses. A function is compilable exactly
/// when this set is a subset of what the generator emits, so this is the
/// progress meter: how much of a real module we can already translate.
```

## L250 · `#[derive(Default, Debug)]`

```
/// The module shape the code generator needs before it emits a byte.
```

## L253 · `pub types: Vec<(Vec<ValType>, Vec<ValType>)>,`

```
/// (params, results) per type index.
```

## L255-256 · `pub imported_funcs: Vec<(String, String)>,`

```
/// `module::name` per imported function, in index order — these occupy
/// the low function indices, ahead of the defined ones.
```

## L258 · `pub funcs: Vec<u32>,`

```
/// Type index per *defined* function, in index order.
```

## L260-261 · `pub func_type_of: Vec<u32>,`

```
/// Type index per function index across the WHOLE space — imports first,
/// then defined. What a `call` needs to know the callee's shape.
```

## L265-266 · `pub global_types: Vec<(ValType, bool)>,`

```
/// Type and mutability per global, imported ones first — they hold the
/// low indices, exactly like functions.
```

## L268-269 · `pub global_init: Vec<Option<i64>>,`

```
/// Constant initialiser per defined global, widened to i64. `None` where
/// the initialiser is not a plain constant.
```

## L275-276 · `pub data_init: Vec<(u32, Vec<u8>)>,`

```
/// Active data segments as (offset, bytes) — what an instance must copy
/// into linear memory before the first instruction runs.
```

## L278-279 · `pub elem_init: Vec<(u32, Vec<u32>)>,`

```
/// Active element segments as (offset, function indices) — what fills the
/// table.
```

## L281-284 · `pub sig_id: Vec<u32>,`

```
/// Canonical signature id per type index. wasm compares function types
/// STRUCTURALLY, so two different type indices with the same shape must
/// pass the same `call_indirect` check — comparing raw type indices would
/// reject calls the spec allows.
```

## L290-291 · `pub fn first_defined(&self) -> u32 {`

```
/// Function index of the first *defined* function; everything below is an
/// import and is called through the host, not with a near call.
```

## L309-311 · `fn const_init(expr: &wasmparser::ConstExpr<'_>) -> Option<i64> {`

```
/// A global's initialiser, when it is a single constant. Anything else —
/// `global.get` of an imported global, say — comes back `None` rather than a
/// guess.
```

## L328-330 · `fn canonical_sig_ids(types: &[(Vec<ValType>, Vec<ValType>)]) -> Vec<u32> {`

```
/// Give structurally identical function types the same id, so a
/// `call_indirect` check compares shapes and not the order the types happened
/// to be written in.
```

## L350 · `pub struct CompiledModule {`

```
/// A module after code generation. `funcs` runs parallel to `plan.bodies`.
```

## L354-357 · `pub code: Vec<u8>,`

```
/// Every compiled function laid out end to end, with a trap stub last.
/// Calls are near-relative, so the functions of one module have to share
/// one object — that is what makes a call a single instruction instead of
/// a load and an indirect jump.
```

## L359-361 · `pub offsets: Vec<Option<usize>>,`

```
/// Offset into `code` per DEFINED function; `None` where generation
/// refused. A call to one of those is aimed at the trap stub, so a
/// half-translated module cannot quietly run into the wrong place.
```

## L363-364 · `pub entry_offset: usize,`

```
/// Where the entry trampoline sits. Native code enters a module here, not
/// at a function directly.
```

## L366 · `pub trap_routine: usize,`

```
/// Where the trap routine sits.
```

## L368-375 · `pub pf_entry: usize,`

```
/// Where a page fault and a divide fault should be sent. Each is two
/// instructions that name the reason and fall into the trap routine.
///
/// Why per reason rather than one entry plus a register: a fault handler
/// can rewrite the interrupted instruction pointer with almost nothing —
/// the saved value sits in the interrupt frame. Rewriting a general
/// register means unpicking however the handler saved them. Two extra
/// stubs cost ten bytes and remove that problem entirely.
```

## L378-380 · `pub trap_offset: usize,`

```
/// Where the trap stub sits. A table slot whose function did not compile
/// points here, so a half-translated module traps instead of jumping into
/// whatever happens to follow.
```

## L385-386 · `pub fn offset_of(&self, function_index: u32) -> Option<usize> {`

```
/// Offset of a function by its index in the whole space. Imports have no
/// code here.
```

## L396-397 · `pub fn compile(wasm: &[u8]) -> Result<CompiledModule, Error> {`

```
/// Validate and generate. A function that meets an opcode the generator does
/// not emit comes back as `Unsupported(name)` — never as wrong code.
```

## L415-416 · `struct Linked {`

```
/// Place every compiled function in one buffer and resolve the `call rel32`
/// displacements that generation had to leave blank.
```

## L427-435 · `struct Linker {`

```
/// Places functions as they come out of the generator, instead of collecting
/// them all and concatenating at the end.
///
/// The difference is not tidiness. Holding every function's buffer until the
/// end means thousands of live blocks in a heap whose free list is walked on
/// every allocation — and on the device that turned a translation that scales
/// linearly with output size into one that scales eight times worse. Taking
/// each function's bytes immediately, and letting its buffer go, keeps the
/// number of live blocks roughly flat no matter how large the module is.
```

## L439 · `relocs: Vec<(usize, u32)>,`

```
/// (offset of the rel32, target function index) — absolute in `code`.
```

## L449-452 · `fn new(wasm_len: usize) -> Linker {`

```
/// `wasm_len` only sizes the first allocation. Generated code runs between
/// one and a half and two and a half times the whole module's bytes, so
/// twice is a guess that usually holds and never costs more than one
/// growth if it does not.
```

## L466-468 · `let mut fault_entry = |code: &mut Vec<u8>, reason: u32| -> usize {`

```
// One entry per processor fault the generator relies on. A handler
// only has to point the interrupted instruction pointer at the right
// one; the entry names the reason itself.
```

## L496 · `fn push(&mut self, out: &mut codegen::Outcome) {`

```
/// Take one function's bytes and let go of its buffer.
```

## L511-512 · `cf.code = Vec::new();`

```
// Handed over: the module owns these bytes now, and one fewer live
// block is one fewer stop on every future walk of the free list.
```

## L519-520 · `let trap = self.code.len();`

```
// Where a call to a function the generator refused lands. It reports
// itself like any other trap instead of faulting.
```

## L560-564 · `pub fn plan(wasm: &[u8]) -> Result<ModulePlan, Error> {`

```
/// The loop the code generator will live in: read an operator, hand it to the
/// validator, then emit. Here it only measures — but the shape is final.
/// Validate a module against `features()` and collect what the code generator
/// needs. One pass: the validator's type stack is the same one the register
/// allocator will ride on, so nothing is walked twice.
```

## L576-577 · `let mut ops_buf: Vec<Operator> = Vec::new();`

```
// One buffer for the operator list, reused for every function. A fresh
// one per function is ten thousand allocations that say nothing.
```

## L590-591 · `let wasmparser::CompositeInnerType::Func(ft) = &ty.composite_type.inner`

```
// Never `unwrap_func()` — it panics, and this input
// is not trusted.
```

## L705-706 · `l.push(&mut o);`

```
// Straight into the module buffer, and the function's own
// buffer goes back to the heap right away.
```

## L717-718 · `fn generate_one(`

```
/// Hand one validated body to the generator, with the signature and the local
/// declarations it needs. Anything missing is a refusal, not a guess.
```

## L786-787 · `Operator::Unreachable`

```
// Every edge out of a basic block, plus every call: the points
// where a register-held fuel counter has to reach memory.
```

## L821-822 · `fn memarg_of<'a>(op: &'a Operator<'_>) -> Option<&'a wasmparser::MemArg> {`

```
/// Accesses to linear memory — the ones that ride the guard-page reservation
/// and therefore need no bounds-check instruction of their own.
```

