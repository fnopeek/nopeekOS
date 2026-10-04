# `forge/harness/src/selftest.rs` @ 5e0102684

## L1-8 · `use std::ffi::c_void;`

```
//! Differential test: the same function under wasmi and under forge, same
//! arguments, and the results must be identical. wasmi is the oracle because
//! it is what the device runs today — a disagreement is a real disagreement,
//! not a spec argument.
//!
//! The generated code is mapped W^X (write, then flip to execute) the way the
//! kernel will have to map it. Doing it right here keeps the harness honest
//! about what the real thing costs.
```

## L12 · `pub(crate) struct Exec {`

```
/// A mapped, executable code page. Dropping it unmaps.
```

## L24 · `let ptr = unsafe {`

```
// SAFETY: anonymous private mapping, size is page-rounded and nonzero.
```

## L38 · `unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), ptr as *mut u8, code.len()) };`

```
// SAFETY: `ptr` is a fresh writable mapping of at least `code.len()`.
```

## L40-41 · `if unsafe { libc::mprotect(ptr, len, libc::PROT_READ | libc::PROT_EXEC) } != 0 {`

```
// W^X: writable while filling, executable afterwards, never both.
// SAFETY: same mapping, same length.
```

## L43 · `unsafe { libc::munmap(ptr, len) };`

```
// SAFETY: unmapping the mapping we just made.
```

## L54 · `pub(crate) fn arm_traps(&self, m: &forge_core::CompiledModule) {`

```
/// Let the fault handler know this code may trap, and where its traps go.
```

## L64-66 · `pub(crate) fn call_entry(`

```
/// Enter the module. Generated functions expect the instance already set
/// up in their pinned registers, so a native caller goes in through the
/// module's trampoline rather than jumping at a function directly.
```

## L76-77 · `unsafe {`

```
// SAFETY: both offsets come from the module's own tables, and the
// trampoline's shape is fixed by `codegen::emit_entry`.
```

## L89 · `unsafe { libc::munmap(self.ptr, self.len) };`

```
// SAFETY: our own mapping, unmapped once.
```

## L94 · `pub(crate) const PAGE: usize = 64 * 1024;`

```
/// One wasm page.
```

## L96-97 · `pub(crate) const DEFAULT_FUEL: i64 = i64::MAX / 4;`

```
/// A budget far above anything the cases need, so the run finishes and what
/// it actually cost is what gets reported.
```

## L99-106 · `pub(crate) const RESERVATION: usize = 8 * 1024 * 1024 * 1024 + PAGE;`

```
/// Address space reserved per instance, and the number has to be exact.
///
/// A wasm address is a u32 and a memory offset is a u32, so the highest
/// effective address is `2^32-1 + 2^32-1 = 2^33-2` — just under 8 GiB. But the
/// ACCESS still has a width: an eight-byte load there reaches `2^33+5`. Eight
/// gibibytes on the nose would therefore leave the last few bytes of the
/// widest access hanging outside the reservation, which is precisely the hole
/// an attacker would look for. One spare page covers every access width.
```

## L109-110 · `pub(crate) struct Memory {`

```
/// Linear memory: 8 GiB of address space, of which only `size` bytes are
/// readable. Costs no physical memory — `PROT_NONE` pages are never backed.
```

## L119 · `let base = unsafe {`

```
// SAFETY: a fresh anonymous reservation, no backing requested.
```

## L133 · `if unsafe { libc::mprotect(base, size, libc::PROT_READ | libc::PROT_WRITE) } != 0 {`

```
// SAFETY: the first `size` bytes of our own reservation.
```

## L135 · `unsafe { libc::munmap(base, RESERVATION) };`

```
// SAFETY: unmapping the reservation we just made.
```

## L145 · `unsafe { libc::munmap(self.base as *mut libc::c_void, RESERVATION) };`

```
// SAFETY: our own reservation, unmapped once.
```

## L150-153 · `pub(crate) struct Inst {`

```
/// The instance context generated code reads through its pinned register,
/// laid out by `forge_core::vmctx`. The backing buffers live here so their
/// addresses stay valid for the call — a `Vec`'s heap block does not move
/// when the `Vec` does, so taking the pointer before the move is sound.
```

## L169-170 · `pub(crate) fn new(m: &forge_core::CompiledModule, code_base: u64) -> Option<Inst> {`

```
/// Fresh per call, so forge starts from the same globals, memory image and
/// table wasmi does.
```

## L174 · `let imported = plan.global_types.len().saturating_sub(plan.global_init.len());`

```
// Imported globals hold the low indices and have no initialiser here.
```

## L178 · `globals.push(0); // never hand out a null base`

```
// never hand out a null base
```

## L187 · `unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.base.add(off), bytes.len()) };`

```
// SAFETY: bounds checked against the readable window above.
```

## L191-193 · `let slots = plan.table.map(|(min, _)| min as usize).unwrap_or(0);`

```
// The table: one code pointer per slot, and the canonical signature id
// beside it. A slot whose function did not compile points at the trap
// stub rather than at nothing.
```

## L212-214 · `let host_fns: Vec<u64> = plan`

```
// Host functions, in import order. An import this harness does not
// know gets the trap stub, so a wrong name faults instead of calling
// something arbitrary.
```

## L234-235 · `ctx[v::MEM_MAX_PAGES as usize / 8] =`

```
// wasm's own ceiling when the module declares none is 65536 pages,
// and the reservation is sized to hold all of it.
```

## L254-257 · `pub(crate) fn trap_code(&self) -> u32 {`

```
/// What is left of the budget. The generated code keeps the counter in a
/// register while it runs and writes it back on the way out, so this is
/// only meaningful after a call has returned.
/// What stopped the last call — `trap::NONE` if it returned normally.
```

## L270 · `pub(crate) fn set_fuel(&mut self, v: i64) {`

```
/// Set a budget small enough to run out.
```

## L276-277 · `extern "C" fn h_add(_ctx: *const u64, a: u32, b: u32, _c: u32) -> u32 {`

```
// The host side, defined once and handed to both engines so a disagreement
// can only come from the generated code.
```

## L288-291 · `pub(crate) extern "C" fn h_memory_grow(ctx: *mut u64, delta: u32) -> u32 {`

```
/// The runtime side of `memory.grow`. The base never moves — that is what the
/// 8 GiB reservation buys — so growing is only ever "make more of it
/// readable". Fresh pages come out zero because the reservation was never
/// written to.
```

## L294-295 · `unsafe {`

```
// SAFETY: `ctx` is the instance context this module was called with, laid
// out by `forge_core::vmctx`.
```

## L355-361 · `pub fn oneshot(wasm: &[u8], arg: u32, fuel: Option<i64>) -> Option<(u32, u32)> {`

```
/// Run `src`'s exported `f` with one argument and report both what it returned
/// and what stopped it.
///
/// Traps used to be checked in a forked child, by watching which signal killed
/// it. They do not need a child any more, and that is the point of this whole
/// round: a trap is a RESULT now. The test can look at the reason instead of a
/// death certificate, and a wrong reason is as visible as a missing one.
```

## L391 · `fn traps_report_themselves() -> bool {`

```
/// Every trap the generator can raise, checked for the RIGHT reason.
```

## L409-410 · `let mem = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \`

```
// A guard page catches an access past the end of memory — no bounds check
// is emitted for it, so this is where that decision is proved.
```

## L419 · `let ind = "(module \`

```
// The table index is the one bound that has to be checked in code.
```

## L440-441 · `let f16 = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \`

```
// Bulk memory checks its own range, because the trap has to come before
// the first byte moves. Zero length still counts.
```

## L454 · `let by = |op: &str| {`

```
// Division: the processor raises the fault, and no compare is emitted.
```

## L469 · `want(&minmax("i32.rem_s"), 0xFFFF_FFFF, None, NONE, "INT_MIN%-1 darf NICHT trappen");`

```
// …and the one that must NOT trap, because wasm wants an answer there.
```

## L472 · `let unr = "(module (func (export \"f\") (param i32) (result i32) \`

```
// `unreachable` is a trap the generator raises itself.
```

## L478 · `let loopy = "(module (func (export \"f\") (param i32) (result i32) \`

```
// A resource limit that does not limit is decoration.
```

## L493 · `decl: &'static str,`

```
/// Module-level text placed before the function — globals, mostly.
```

## L510-511 · `const CASES: &[Case] = &[`

```
/// Bodies are written against `local.get 0..n`. Two parameters unless the
/// case says otherwise.
```

## L548-549 · `Case { name: "shl_big", decl: "", body: "local.get 0 i32.const 33 i32.shl", params: 1 },`

```
// A count above 31 is taken mod 32 by both wasm and x86 — worth pinning
// down rather than assuming.
```

## L553 · `Case { name: "block_empty", decl: "", body: "(block) local.get 0", params: 1 },`

```
// --- structured control flow ---
```

## L568-569 · `Case { name: "brif_value", decl: "", body:`

```
// The value a taken br_if carries must not overwrite an operand that is
// still live on the fall-through path.
```

## L602 · `Case { name: "global_get", decl: "(global $g (mut i32) (i32.const 1234))", body:`

```
// --- globals ---
```

## L612-614 · `Case { name: "mem_st_ld", decl: "(memory 1)", body:`

```
// --- linear memory ---
// Every address is masked so the case stays inside the mapped window; the
// guard-page check below is what proves the rest of the reservation bites.
```

## L653 · `Case { name: "mem_loop", decl: "(memory 1)", body:`

```
// A loop that walks memory — the shape every real module has.
```

## L671 · `Case { name: "call_1", decl: "(func $g (param i32) (result i32) local.get 0 i32.const 3 i32.mul)",`

```
// --- calls ---
```

## L685-686 · `Case { name: "call_rec", decl:`

```
// Recursion: the callee's frame must not disturb the caller's, and the
// pinned registers have to survive the whole way down and back.
```

## L692-693 · `Case { name: "call_under", decl:`

```
// An operand sits BELOW the arguments: they must be read from the right
// slots, not from the bottom of the stack.
```

## L701-702 · `Case { name: "call_mem", decl:`

```
// The callee writes memory the caller then reads — the pinned memory base
// has to be right on both sides of the call.
```

## L708 · `Case { name: "host_add", decl:`

```
// --- host imports ---
```

## L723 · `Case { name: "call_ind", decl:`

```
// --- call_indirect ---
```

## L738-740 · `Case { name: "call_ind_same", decl:`

```
// Two DISTINCT type indices with the same shape. wasm compares types
// structurally, so this must pass — comparing raw type indices would
// reject it, and nothing else in the suite would notice.
```

## L749 · `Case { name: "select", decl: "", body:`

```
// --- select ---
```

## L759-761 · `Case { name: "brt_3", decl: "", body:`

```
// --- br_table ---
// Named labels throughout: hand-counted branch depths are how a test file
// grows an infinite loop that looks like a compiler bug.
```

## L772 · `Case { name: "brt_default", decl: "", body:`

```
// An index past the last target really has to reach the default.
```

## L782 · `Case { name: "brt_value", decl: "", body:`

```
// A br_table carrying a value, to labels at DIFFERENT depths.
```

## L790 · `Case { name: "brt_dup", decl: "", body:`

```
// Duplicate targets: two table entries pointing at the same label.
```

## L810 · `Case { name: "brt_loop", decl: "", body:`

```
// A table inside a loop — the back edge must stay a plain jump.
```

## L825 · `Case { name: "call_6", decl:`

```
// --- more than five arguments ---
```

## L843 · `Case { name: "call_ind_7", decl:`

```
// An indirect call with stack arguments.
```

## L850-851 · `Case { name: "call_6_loop", decl:`

```
// A call with stack arguments inside a loop: `rsp` has to come back every
// single time round, or the frame walks away.
```

## L865-874 · `Case { name: "i64_add", decl: "", body:`

```
// --- i64 ---
//
// The exported function keeps its i32 shape; i64 flows through locals,
// globals, memory and inner calls. Two idioms recur:
//   MK   builds a 64-bit value with arg0 in the high half and arg1 in the
//        low half, so both halves carry real data
//   FOLD xors the two halves back into an i32, so a wrong upper half
//        cannot hide behind a truncating result
// FOLD names local 2, so every case using it declares two parameters —
// even where the second one is not read.
```

## L931 · `Case { name: "i64_eq", decl: "", body:`

```
// Comparisons: i64 in, i32 out.
```

## L957-958 · `Case { name: "i64_clz", decl: "", body:`

```
// Bit counting. The zero input is the case `bsr`/`bsf` leave undefined,
// so it gets its own case rather than riding on the argument table.
```

## L972-974 · `Case { name: "i64_popcnt", decl: "", body:`

```
// Built from two i32 params like the i64 clz/ctz cases above, so the
// HIGH half is exercised too — a `popcnt` emitted without REX.W counts
// only the low 32 bits and would pass a test that never sets them.
```

## L981 · `Case { name: "i64_ext_u", decl: "", body:`

```
// Width conversions, both directions and both signednesses.
```

## L1012 · `Case { name: "i64_mem", decl: "(memory 1)", body:`

```
// --- i64 in memory ---
```

## L1052 · `Case { name: "i64_global", decl: "(global $g (mut i64) (i64.const 0x0123456789abcdef))", body:`

```
// --- i64 through globals, select, blocks and calls ---
```

## L1078 · `Case { name: "i64_call_6", decl:`

```
// Six i64 parameters: the sixth goes on the stack, at full width.
```

## L1086 · `Case { name: "i64_call_mix", decl:`

```
// Mixed widths in one signature, across the register/stack boundary.
```

## L1098-1103 · `Case { name: "mem_fill", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz012`

```
// --- bulk memory ---
//
// The memory is seeded from a data segment with 64 distinguishable bytes,
// and the check folds position-dependently (`acc*31 + byte`) over a window
// wider than anything the cases touch. A plain sum would let a copy that
// ran in the wrong direction pass.
```

## L1110 · `Case { name: "mem_cp_fwd", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0`

```
// Destination below the source: copying upwards is safe.
```

## L1113-1114 · `Case { name: "mem_cp_bwd", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0`

```
// Destination ABOVE the source and overlapping: this is the case a plain
// forward copy gets wrong, and the only one the direction flag is for.
```

## L1128-1131 · `Case { name: "i32_div_s", decl: "", body:`

```
// --- division ---
// The divisor is forced non-zero, and for the signed quotients also away
// from -1, so the ordinary cases can share the argument table with
// everything else. The trapping combinations are checked in a child.
```

## L1138-1139 · `Case { name: "i32_rem_s", decl: "", body:`

```
// Here the divisor MAY be -1: the argument table has 0xFFFFFFFF paired
// with INT_MIN, which is exactly the shortcut's reason to exist.
```

## L1146-1147 · `Case { name: "i32_div_trunc", decl: "", body:`

```
// Truncation runs toward zero and the remainder keeps the dividend's
// sign — the two places where a hand-written lowering usually drifts.
```

## L1179-1185 · `Case { name: "f64_add", decl: "", body:`

```
// --- floating point ---
//
// A float result is NOT compared bit for bit: wasm leaves NaN payloads to
// the implementation, so forge and wasmi may legitimately differ there.
// The fold below asks whether the result IS a NaN — the part the spec does
// fix — and folds a non-NaN result down through its exact bits, so signed
// zero and the last mantissa bit still count.
```

## L1202 · `Case { name: "f64_add_bits", decl: "", body:`

```
// Inputs built from raw bits, so infinities, NaNs and denormals get in.
```

## L1207 · `Case { name: "f64_min", decl: "", body:`

```
// min/max: the two places the hardware instruction disagrees with wasm.
```

## L1230-1231 · `Case { name: "f64_floor", decl: "", body:`

```
// Rounding. `nearest` rounds halves to EVEN, which is the one a
// hand-written lowering usually gets wrong in both directions.
```

## L1252 · `Case { name: "f64_abs", decl: "", body:`

```
// Sign work, where the answer is a bit pattern and signed zero decides.
```

## L1269-1270 · `Case { name: "f64_cmp_eq", decl: "", body:`

```
// Comparisons return i32, so they need no fold — and an unordered pair
// is where `setb` would call a NaN "less than".
```

## L1309 · `Case { name: "f64_cv_i32s", decl: "", body:`

```
// Conversions in both directions and both signednesses.
```

## L1338-1340 · `Case { name: "ts_i32f64s", decl: "", body:`

```
// Saturating truncation: the hardware answers "indefinite" for a NaN, for
// either overflow AND for a legitimate minimum, so every one of those
// gets its own case.
```

## L1377 · `Case { name: "f64_mem", decl: "(memory 1)", body:`

```
// Floats through memory, globals, calls and select.
```

## L1397-1398 · `Case { name: "grow_size", decl: "(memory 1 4)", body:`

```
// --- memory.grow ---
// The base must not move across a grow, and the new pages must read zero.
```

## L1407-1408 · `Case { name: "grow_keeps", decl: "(memory 1 4)", body:`

```
// Write below the old end, grow, then read it back — the pinned base has
// to still be right afterwards.
```

## L1413 · `Case { name: "grow_fresh", decl: "(memory 1 4)", body:`

```
// Fresh pages read zero, and are writable.
```

## L1420 · `Case { name: "global_sp", decl: "(global $sp (mut i32) (i32.const 65536))", body:`

```
// The shape python leans on: a stack pointer taken down and put back.
```

## L1455-1456 · `let Some(&(_, fidx)) = m.plan.exports.iter().find(|(n, _)| n == "f") else {`

```
// Which function is `f`, and did it compile? With calls in the picture
// a module is more than one function, so both questions are real.
```

