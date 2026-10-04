# `forge/core/src/codegen.rs` @ 5e0102684

## L1-11 · `use crate::trap;`

```
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
```

## L19-23 · `const ARG_REGS: [Reg; 5] = [Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];`

```
/// Where a wasm function's arguments arrive. `rdi` carries the instance
/// context, so the integer arguments start one register later than SysV.
/// Anything past these goes on the stack, exactly as SysV does it: the caller
/// writes them at `[rsp + 8*j]` before the call, so the callee finds them at
/// `[rbp + 16 + 8*j]` once the return address and its own `rbp` are pushed.
```

## L26 · `fn incoming_arg(j: usize) -> i32 {`

```
/// Frame offset of the `j`-th stack argument as the CALLEE sees it.
```

## L31-32 · `fn outgoing_bytes(n: usize) -> i32 {`

```
/// Bytes the caller must reserve for `n` stack arguments, keeping `rsp`
/// 16-aligned across the call.
```

## L37 · `const A: Reg = Reg::Rax;`

```
/// Scratch. None holds a live value across an operator boundary.
```

## L41-42 · `const T: Reg = Reg::R11;`

```
/// A fourth scratch that is NOT an argument register, so a call target can be
/// held while the arguments are loaded on top of `rcx` and `rdx`.
```

## L45-47 · `const VMCTX: Reg = Reg::R14;`

```
/// The instance context, pinned for the whole function. Callee-saved in SysV,
/// so the frame saves and restores it and generated code may treat it as
/// constant.
```

## L50-60 · `const FUEL: Reg = Reg::R15;`

```
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
```

## L63-67 · `const MEMBASE: Reg = Reg::R13;`

```
/// It never has to be reloaded: the instance reserves its address space once
/// and `memory.grow` only makes more of that same range readable, so the base
/// is fixed for the instance's life. An implementation that moved memory on
/// growth would have to reload this after every call — ours does not, and the
/// guard-page reservation is exactly why.
```

## L70-74 · `const RESERVED_SLOTS: u32 = 0;`

```
/// No frame slots are reserved any more. `r13` and `r14` used to be saved and
/// restored in every function; they are now set ONCE by the entry trampoline
/// and are the same for every function of an instance, so saving them per call
/// was six instructions of pure ceremony. Only the boundary to native code
/// needs them preserved, and that is exactly what the trampoline is.
```

## L77-79 · `pub struct Reloc {`

```
/// A `call rel32` whose target had no address yet. `at` is the offset of the
/// displacement inside this function's own code; `target` is a wasm function
/// index. The module linker resolves both once every function is placed.
```

## L85-86 · `pub struct ModuleCtx<'a> {`

```
/// Everything the generator needs to know about the module around the
/// function it is translating.
```

## L89 · `pub func_type_of: &'a [u32],`

```
/// Type index per function index, imports first.
```

## L92 · `pub sig_id: &'a [u32],`

```
/// Canonical signature id per type index.
```

## L97-108 · `pub fn emit_trap_routine(asm: &mut Asm) {`

```
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
```

## L119-121 · `asm.sub_r64_imm32(Reg::Rsp, 32);`

```
// A fixed frame instead of pushes: three callee-saved registers would
// leave the stack misaligned for the call, and named offsets read better
// than counting pushes.
```

## L129 · `asm.store64(VMCTX, vmctx::TRAP_RBP, Reg::Rbp);`

```
// Where a trap should come back to, recorded before anything can trap.
```

## L140 · `asm.mov_rr64(Reg::Rax, Reg::Rsi); // the function to enter`

```
// the function to enter
```

## L141 · `asm.mov_rr64(Reg::Rsi, Reg::Rdx); // and its arguments, shifted down`

```
// and its arguments, shifted down
```

## L146 · `let resume = asm.pos();`

```
// Both the ordinary return and every trap arrive here.
```

## L150 · `asm.store64(VMCTX, vmctx::FUEL, FUEL);`

```
// What is left goes back where the runtime can read it.
```

## L161-169 · `pub code: Vec<u8>,`

```
/// The generated bytes — and EMPTIED as soon as the module linker has
/// taken them.
///
/// A module of ten thousand functions must not hold ten thousand live
/// buffers: the kernel's heap keeps its free blocks on a singly linked
/// list, so every allocation walks past everything still alive. Measured
/// on the device, that was the whole story — translating scaled linearly
/// with output size on the development machine and 8x worse than linear
/// there, and the only difference between the two runs is that allocator.
```

## L172 · `pub trap_relocs: Vec<usize>,`

```
/// Offsets of `jmp rel32`s aimed at the module's trap routine.
```

## L174 · `pub frame: u32,`

```
/// Bytes of frame below `rbp`, already 16-aligned.
```

## L176 · `pub max_stack: u32,`

```
/// Deepest the operand stack got.
```

## L182 · `Unsupported(&'static str),`

```
/// The opcode that stopped it. Naming it is the whole point.
```

## L188 · `Func,`

```
/// The function body itself. Branching to it leaves the function.
```

## L197 · `height: u32,`

```
/// Operand stack height when the frame was entered.
```

## L199-200 · `branch_arity: u32,`

```
/// Values a branch to this label carries. A loop's label is its head, and
/// without multi-value a loop has no parameters — so zero.
```

## L202-203 · `out_arity: u32,`

```
/// Values left on the stack when control falls out of the frame, and of
/// what type — the operand stack has to be rebuilt exactly at every end.
```

## L206 · `start: usize,`

```
/// Loop only: code position of the head, the target of a back edge.
```

## L208 · `ends: Vec<Patch>,`

```
/// Forward branches waiting for this frame's end.
```

## L210 · `else_patch: Option<Patch>,`

```
/// `If` only: the edge taken when the condition is false.
```

## L212 · `dead: bool,`

```
/// Was the frame entered from unreachable code? Then it emits nothing.
```

## L216-217 · `fn w64(t: ValType) -> bool {`

```
/// Does this value need REX.W — that is, is it a 64-bit INTEGER? Floats never
/// answer yes here; their width is carried by `Fw`.
```

## L222-224 · `fn wide(t: ValType) -> bool {`

```
/// Does the value occupy all eight bytes of its slot? True for i64 AND f64,
/// which is a different question from `w64` and used in different places —
/// conflating the two is how an f64 ends up half-stored.
```

## L237-239 · `const FA: Xmm = Xmm::X0;`

```
/// Scratch float registers. `X0` doubles as the float return register and the
/// first float argument register, which is harmless: arguments are loaded
/// immediately before a call and the result is taken immediately after.
```

## L244-247 · `const FARG_REGS: [Xmm; 8] = [`

```
/// Float arguments have their OWN sequence of registers in SysV, counted
/// separately from the integer ones. A signature of (i32, f64, i32) puts the
/// integers in the first two integer slots and the double in `xmm0` — not in
/// "the second argument register".
```

## L252 · `#[derive(Copy, Clone)]`

```
/// Where one parameter travels.
```

## L257 · `Stack(usize),`

```
/// Index of the eight-byte stack slot, counted from the first one.
```

## L261-262 · `fn arg_places(params: &[ValType]) -> (Vec<Place>, usize) {`

```
/// Assign every parameter its place. Both sides of a call run this, so caller
/// and callee cannot disagree about where an argument is.
```

## L286-288 · `#[derive(Copy, Clone, PartialEq)]`

```
/// Where an operand-stack value actually is. `Slot` means it has been written
/// to its own frame slot, which is the canonical place and the only one a
/// branch target or a callee may assume.
```

## L294 · `Imm(i64),`

```
/// A constant that has not been put anywhere yet.
```

## L296-305 · `Local(u32),`

```
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
```

## L315-328 · `const GPRS: [Reg; 5] = [Reg::Rsi, Reg::Rdi, Reg::R8, Reg::R9, Reg::R10];`

```
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
```

## L336-337 · `gpr_used: [bool; 16],`

```
/// Which registers are spoken for — by a stack value, or by an operator
/// that has taken one out and not given it back yet.
```

## L340-341 · `local_types: Vec<ValType>,`

```
/// Type of every local, parameters first. A local's slot holds a value of
/// exactly this width — nothing reads the bytes above it.
```

## L344-346 · `traps: Vec<(u32, Vec<Patch>)>,`

```
/// Branches out of the function, grouped by what went wrong. Each group
/// gets its own two-instruction stub after the epilogue, so the common
/// path falls straight through and the reason survives to the runtime.
```

## L348-349 · `trap_relocs: Vec<usize>,`

```
/// Offsets of the `jmp rel32`s in those stubs; the module linker points
/// them at the trap routine.
```

## L352-353 · `stack: Vec<Val>,`

```
/// The operand stack. Its LENGTH is the depth — one source of truth, so a
/// push that forgets its type cannot happen.
```

## L358-361 · `fuel_pending: i64,`

```
/// Instructions counted since the last time the fuel register was
/// updated. Charged in one go at the next control-flow edge, so metering
/// costs one `sub` per basic block — measured at 5,8 wasm instructions —
/// instead of one per instruction.
```

## L363-365 · `reachable: bool,`

```
/// False after a branch, until an `else` or an `end` brings control back.
/// Unreachable code is not emitted: the validator types it
/// polymorphically, so tracking a stack through it would be fiction.
```

## L370-379 · `pub mod census {`

```
// ── Auszaehlung: wohin gehen die erzeugten Bytes? ─────────────────────
//
// Haengt am schon vorhandenen Merkmal `census` (Host-Werkzeug), damit im
// Kernel weder Zaehler noch Atomics landen. Ohne das Merkmal sind die drei
// Funktionen leer und verschwinden.
//
// Die Frage war, ob der Abstand zu Cranelift (1,43x auf python) an der
// Registerhaltung ueber Blockgrenzen haengt: forge muss vor jedem
// Loop/If/Else/End/Br/BrIf/BrTable/Return und jedem Aufruf den ganzen
// Wertestapel in Schlitze schreiben.
```

## L418-420 · `#[inline(always)]`

```
/// Argumente eines Aufrufs, aus ihren Schlitzen geladen. Der zweite Leser
/// neben `pop_to` — ohne ihn sieht die Spill-Bilanz aus, als wuerde
/// niemand zurueckholen, was `spill_all` wegschreibt.
```

## L439 · `#[cfg(feature = "census")]`

```
/// (spill_b, spill_n, reload_b, reload_n, local_b, local_n, arg_b, arg_n)
```

## L450 · `fn local(&self, i: u32) -> i32 {`

```
/// Frame slot for local `i`, relative to `rbp`.
```

## L455-457 · `fn slot(&self, d: u32) -> i32 {`

```
/// Frame slot for operand-stack entry `d`. Every value has one whether it
/// is currently in a register or not — a spill needs somewhere to go, and
/// the place must not depend on when the spill happens.
```

## L470 · `fn push_imm(&mut self, v: i64, t: ValType) {`

```
/// A constant, remembered rather than emitted.
```

## L476 · `fn push_local(&mut self, idx: u32, t: ValType) {`

```
/// A copy of a local, remembered rather than loaded.
```

## L482-483 · `fn settle_local(&mut self, idx: u32) {`

```
/// Settle every pending reference to local `idx` — it is about to change,
/// and wasm took its copy earlier.
```

## L492-493 · `fn push_slot(&mut self, t: ValType) {`

```
/// Note a value that is already in its slot — what a block end leaves
/// behind, since everything is spilled before control can join.
```

## L499 · `fn alloc_gpr(&mut self) -> Reg {`

```
// --- registers ---
```

## L508-509 · `self.spill_deepest_gpr();`

```
// Nothing free: the value that has waited longest goes to memory.
// Spilling the deepest keeps the ones an operator is about to want.
```

## L544-548 · `fn spill(&mut self, i: usize) {`

```
/// Write the value at `i` to its own slot and let go of its register.
///
/// Materialising through `r11`: it is neither a cache register nor
/// anything an operator holds while a spill can happen — spills come from
/// `alloc_gpr` and from `spill_all`, and both run between operators.
```

## L609-612 · `fn spill_all(&mut self) {`

```
/// Put the whole operand stack where everyone else expects to find it.
/// Mandatory before a call (the registers do not survive it) and at every
/// point where control can join — a branch target cannot know which
/// register a value happened to be in on the way there.
```

## L619-626 · `fn spill_below(&mut self, keep: usize) {`

```
/// Alles UNTER den obersten `keep` Werten in die Schlitze. Vor einem Aufruf
/// ist das die Pflicht — der Aufgerufene besitzt jedes Cache-Register.
///
/// Die Argumente selbst gehoeren NICHT dazu: sie muessen in den
/// Argumentregistern stehen, und der Weg dorthin ueber einen Schlitz ist
/// ein Umweg. Gezaehlt an python: 172 514 Spills und 149 355 Argumentladen
/// waren zusammen 18,2 % der erzeugten Bytes, und der groesste Teil davon
/// war ein Wert, der nur von einem Register in ein anderes wollte.
```

## L634-647 · `fn spill_arg_conflicts(&mut self, places: &[Place], base: usize) {`

```
/// Die Faelle, die ein direkter Zug NICHT kann — hier, wo `r11` noch frei
/// ist. Danach braucht `load_args` weder Spill noch Hilfsregister, und das
/// ist die Bedingung dafuer, dass `call_indirect` sein Sprungziel in `r11`
/// halten darf.
///
/// Drei Sorten gehen in ihren Schlitz:
/// 1. Ein Argument, dessen Register das ZIEL eines ANDEREN Arguments ist —
///    der Zug dorthin wuerde es ueberschreiben. (Ueberschneidung: `rsi`,
///    `r8`, `r9` sind beides.)
/// 2. Fliesskomma-Argumente. Cache- und Argument-XMM ueberschneiden sich
///    ebenfalls; die alte, sichere Form kostet hier wenig, weil die
///    Ganzzahlen die Masse stellen.
/// 3. Argumente, die ueber den Stapel gehen — sie werden aus ihrem Schlitz
///    geschrieben.
```

## L671 · `fn evict_gpr(&mut self, r: Reg) {`

```
/// Make `r` available: whatever stack value holds it goes to its slot.
```

## L690 · `fn truncate_to(&mut self, h: u32) {`

```
/// Drop the stack back to `h` entries, releasing their registers.
```

## L701 · `fn note_depth(&mut self) {`

```
// --- the operand stack ---
```

## L709 · `fn fpush_reg(&mut self, x: Xmm, t: ValType) {`

```
/// The float equivalents of `push_reg` / `pop_to_pool`.
```

## L727-728 · `fn push_reg(&mut self, r: Reg, t: ValType) {`

```
/// The value is already in a cache register and stays there. What the hot
/// operators use, so that an arithmetic result costs no move at all.
```

## L734-736 · `fn pop_to_pool(&mut self) -> Reg {`

```
/// Take the top into a CACHE register and keep it there — in place if it
/// already is one. The caller may write it and hand it straight back with
/// `push_reg`, which is how the last move disappears from the hot path.
```

## L741 · `return r; // still marked used: it belongs to the caller now`

```
// still marked used: it belongs to the caller now
```

## L749-752 · `fn push_from_ty(&mut self, r: Reg, t: ValType) {`

```
/// The operator produced the value in its own scratch register; give it a
/// place the stack can keep it. When every cache register is taken the
/// deepest one goes to memory, which is the case this whole arrangement is
/// trying to make rare.
```

## L779-783 · `fn pop_to(&mut self, r: Reg) -> Option<ValType> {`

```
/// Take the top value into `r`, whatever it costs: nothing if it is
/// already there, a register move if it is elsewhere, a load if it was
/// spilled. The caller owns `r` afterwards and must give it back.
/// Take the top value into `r`. `r` is operator scratch and never holds a
/// stack value, so nothing has to be evicted first.
```

## L796-797 · `self.asm.xmm_to_gpr(wide(v.ty), r, x);`

```
// Only the bits are wanted; this is how a float reaches a
// general register for a slot move or a reinterpret.
```

## L832 · `fn fpop_to(&mut self, x: Xmm) -> Option<ValType> {`

```
/// The same for the float register file.
```

## L860-862 · `fn peek_to(&mut self, r: Reg) {`

```
/// Read the top into `r` without consuming it. The value keeps its own
/// place, so `r` is a copy and the caller must free it.
/// Read the top into `r` without consuming it — the value keeps its place.
```

## L900 · `fn globals_base(&mut self, r: Reg) {`

```
/// Load the globals array base into `r`.
```

## L905-906 · `fn fuel_flush(&mut self) {`

```
/// Charge everything counted so far. Must come BEFORE anything that sets
/// flags for a branch — `sub` writes the flags too.
```

## L916-919 · `fn fuel_check(&mut self) {`

```
/// Is there any fuel left? Only needed where execution could otherwise run
/// forever — a loop's back edge and a function's entry. Straight-line code
/// cannot loop, so it needs no check, and the counter simply goes negative
/// until the next one.
```

## L922-923 · `let p = self.asm.jcc(Cond::Le);`

```
// `jle`, not `js`: a budget of exactly zero is spent too, and a check
// that only fired on negative would let one more block through.
```

## L947-948 · `fn int_ty(t: ValType) -> bool {`

```
/// Every numeric type wasm has. What is left out is the reference types, and
/// `features()` already refuses those before the generator ever sees them.
```

## L956-958 · `fn block_out(ty: BlockType) -> Result<(u32, ValType), &'static str> {`

```
/// Values a block type leaves behind, and of what type. Without multi-value
/// there are only two shapes, and a type index would be one — so it is refused
/// rather than mishandled.
```

## L997-999 · `asm: Asm::with_capacity(ops.len() * 16),`

```
// Roughly what a function of this length comes to, so the per-function
// buffer does not grow either. Measured at 8,5 bytes per wasm
// instruction; sixteen leaves room without wasting much.
```

## L1017-1018 · `f.ctrl.push(Ctrl {`

```
// The function body is a frame like any other, so `br` to the outermost
// depth needs no special case — it is a branch out of the function.
```

## L1031-1032 · `f.asm.push(Reg::Rbp);`

```
// Prologue. The frame size is not known yet, so the immediate is written
// as zero and patched at the end.
```

## L1050-1052 · `let src = incoming_arg(j);`

```
// A stack argument always fills a whole eight-byte slot, so
// the raw bits can be moved through a general register no
// matter what the type is.
```

## L1064-1066 · `if n_declared > 0 {`

```
// Declared locals are zero in wasm, and nothing else may be assumed. The
// full eight bytes are cleared regardless of width — cheaper than deciding
// per local, and it leaves no stale upper half behind.
```

## L1075-1076 · `f.fuel_check();`

```
// Entry check: with one at every loop head and one here, no path can run
// unbounded without meeting one.
```

## L1090 · `if let Some(rt) = results.first().copied() {`

```
// Epilogue. Every path arrives with the result, if any, in slot 0.
```

## L1105-1106 · `let traps = core::mem::take(&mut f.traps);`

```
// One stub per reason, after the return path so the common path falls
// straight through. Two instructions: name the reason, leave.
```

## L1140-1152 · `if f.reachable && !matches!(op, Block { .. } | Loop { .. } | Else | End | Nop) {`

```
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
```

## L1157-1158 · `match op {`

```
// Frames are tracked even where no code is emitted, because the nesting
// is what tells us when control becomes reachable again.
```

## L1181-1182 · `if !dead {`

```
// The head is a branch target: the back edge arrives with
// everything in its slot, so the way in must match.
```

## L1186-1189 · `let head = f.asm.pos();`

```
// Mark the head BEFORE the check, not after. A loop is the only
// place execution can go round for ever, so the check has to sit
// ON the back edge — recorded after it, the edge jumps straight
// past and the loop is checked exactly once, on the way in.
```

## L1197-1198 · `branch_arity: 0,`

```
// A branch to a loop goes to its head and carries the loop's
// parameters. Without multi-value there are none.
```

## L1217-1219 · `f.spill_all();`

```
// Both arms start from this point, and the else-arm is
// reached long after the then-arm has had its way with the
// registers. Settling here is what keeps the two agreeing.
```

## L1246 · `if f.reachable {`

```
// The then-arm, if it can fall out, skips the else-arm.
```

## L1263-1264 · `if f.reachable {`

```
// Control joins here — the fall-through and every branch that
// aimed at this end must leave the operand stack looking the same.
```

## L1272-1273 · `if let Some(ep) = fr.else_patch.take() {`

```
// An `if` with no `else` still needs its false edge to land
// somewhere, and that somewhere is here.
```

## L1346-1347 · `let d = f.depth();`

```
// Dropping has to hand the register back, or the allocator would
// lose one per discarded value.
```

## L1361 · `let t = f.pop_to(A).ok_or("stack-empty")?;`

```
// Only the bits move, so a general register does for floats too.
```

## L1436-1438 · `I32Shl => shift(f, false, 4),`

```
// The shift count lands in `rcx` by construction: `bin` pops the
// right-hand operand there first, and `cl` is where x86 wants it.
// wasm masks the count to the operand's bit width, and so does x86.
```

## L1450-1451 · `I32Eq => cmp(f, false, Cond::E),`

```
// Comparisons take operands of their own width and always leave an
// i32 behind, whatever went in.
```

## L1473-1489 · `I32DivS => div_op(f, false, true, false),`

```
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
```

## L1502-1504 · `I32Clz => clz(f, false),`

```
// `bsr`/`bsf` leave the destination undefined for a zero input and
// say so in ZF. The sentinel is arranged so the SAME final `xor`
// produces wasm's answer, which keeps the whole thing branch-free.
```

## L1514-1517 · `I64Popcnt => {`

```
// The emitter took its width flag from the start; only this arm was
// missing. It is not a niche opcode: a pure-Rust H.264 decoder built
// for wasm32 lands two functions on it, and they carry 23 % of the
// module's instructions.
```

## L1524-1527 · `I32WrapI64 => {`

```
// Width conversions. `pop_to` already read the value at its own
// width, so wrapping needs no instruction at all — the narrower store
// does it, and the zero-extending direction is already done by the
// 32-bit load.
```

## L1567 · `F32Const { value } => f.push_imm(value.bits() as i64, ValType::F32),`

```
// --- floating point ---
```

## L1593 · `F32Nearest => funary(f, Fw::Single, |a, w, x| a.fround(w, 0, x, x)),`

```
// wasm rounds halves to even, which is `round`'s mode 0.
```

## L1597-1599 · `F32Abs => fsign(f, Fw::Single, Sign::Abs),`

```
// Sign work is bit work: clear the sign bit, flip it, or take it from
// the other operand. No arithmetic, so NaNs pass through untouched —
// which is exactly what wasm specifies for these three.
```

## L1631-1633 · `F32ConvertI32S => int_to_f(f, Fw::Single, false),`

```
// Signed int to float is one instruction. Unsigned from i32 is too,
// by widening to i64 first — a u32 always fits. Unsigned from i64 is
// the only one that needs work, below.
```

## L1639 · `f.pop_to(A); // the 32-bit load already zero-extended`

```
// the 32-bit load already zero-extended
```

## L1679-1682 · `MemoryGrow { mem } => {`

```
// The only operator that cannot be done with instructions alone: it
// needs a mapping changed. So it calls the one runtime routine, and
// afterwards the pinned memory base MUST be reloaded — this is the
// case the invariant on MEMBASE was written for.
```

## L1700 · `f.asm.shr_imm(true, A, 16); // bytes to 64 KiB pages`

```
// bytes to 64 KiB pages
```

## L1704-1711 · `MemoryCopy { dst_mem, src_mem } => {`

```
// --- bulk memory ---
//
// The first operators where the guard page is NOT enough. A guard
// catches one access; these walk a RANGE, and the specification wants
// the trap BEFORE the first byte moves. Faulting halfway would already
// have changed memory — observably different from the interpreter.
// So the bounds are checked up front, in 64 bits, where `dst + len`
// cannot overflow because both halves are u32.
```

## L1716-1717 · `f.spill_all();`

```
// `rep movsb` wants rsi/rdi/rcx, and two of those are cache
// registers — so the cache goes to memory first.
```

## L1719 · `f.pop_to(B); // rcx = len`

```
// rcx = len
```

## L1720 · `f.pop_to(Reg::Rsi); // src`

```
// src
```

## L1721 · `f.pop_to(Reg::Rdi); // dst`

```
// dst
```

## L1727-1730 · `f.asm.cmp_rr64(Reg::Rdi, Reg::Rsi);`

```
// wasm's memory.copy is a memmove: overlapping ranges must come
// out right. Copying downwards is safe whenever the destination
// is at or below the source; otherwise the copy has to run
// backwards, which is what the direction flag is for.
```

## L1750 · `f.pop_to(B); // rcx = len`

```
// rcx = len
```

## L1751 · `f.pop_to(A); // al = the byte`

```
// al = the byte
```

## L1752 · `f.pop_to(Reg::Rdi); // dst`

```
// dst
```

## L1758 · `Call { function_index } => {`

```
// --- calls ---
```

## L1771-1782 · `F32Load { memarg } => fload(f, memarg, ValType::F32)?,`

```
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
```

## L1795-1797 · `I64Load8U { memarg } => load(f, memarg, ValType::I64, |a, d, b, i, o| a.load8u_idx(d, b, i, o))?,`

```
// A zero-extending narrow load clears the whole register, so the
// 32-bit form serves i64 unchanged. The sign-extending ones do not:
// filling 64 bits with the sign is a different instruction.
```

## L1808-1809 · `I64Store { memarg } => store(f, memarg, |a, b, i, o, s| a.store64_idx(b, i, o, s))?,`

```
// A narrow i64 store writes the low bytes of the register, which is
// the very same instruction the i32 ones use.
```

## L1815-1817 · `Select => {`

```
// `select` takes the condition last, so the two candidates sit below
// it. `cmov` reads the flags the condition's `test` left, and neither
// `mov` in between disturbs them.
```

## L1819 · `f.pop_to(A); // the condition`

```
// the condition
```

## L1828-1832 · `let w = wide(t);`

```
// Only bits are being chosen, so `cmov` on general registers
// serves floats as well — and avoids a branch on a condition that
// is usually unpredictable.
// Nothing between the `test` and the `cmov` touches the flags:
// moves, loads and stores all leave them alone.
```

## L1834 · `f.pop_to(B); // the value taken when the condition is false`

```
// the value taken when the condition is false
```

## L1835 · `f.pop_to(A); // the value taken when it is true`

```
// the value taken when it is true
```

## L1836-1837 · `f.asm.cmov(w, Cond::E, A, B);`

```
// Zero means false, so `cmovz` is what picks the second value —
// which is why the first one is fetched into the destination.
```

## L1849-1850 · `fn branch_to(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {`

```
/// Move the branch's values into the target frame's slots and jump. A loop's
/// label is its head; every other frame's label is its end.
```

## L1880-1885 · `fn br_table(f: &mut Ctx, t: &wasmparser::BrTable<'_>) -> Result<(), &'static str> {`

```
/// A jump table, not a chain of compares: `br_table` is how a `switch`
/// arrives, and a chain would make the cost depend on which case was taken.
///
/// Layout is dispatch, then one stub per case, then the table itself. The
/// table sits last because every stub ends in a jump, so control can never
/// fall into it — and putting it there means no jump has to skip over it.
```

## L1903-1904 · `let mut stubs = Vec::with_capacity(n);`

```
// Every case gets a stub, because a branch may have to move a value into
// its label's slot before jumping, and that move differs per target.
```

## L1922-1925 · `fn br_if(f: &mut Ctx, relative_depth: u32) -> Result<(), &'static str> {`

```
/// `br_if` cannot move the branch's value before the test: the value's
/// destination slot may still hold a live operand on the fall-through path.
/// With no values to carry it is a single conditional jump; otherwise the
/// move sits on the taken side of a short skip.
```

## L1935-1936 · `f.spill_all();`

```
// The taken side lands on a label, and the fall-through continues from
// here — both have to agree, so settle before either happens.
```

## L1957-1963 · `fn bulk_bounds(f: &mut Ctx, reg: Reg) -> Result<(), &'static str> {`

```
/// `start + len > memory size` traps, where `start` is a zero-extended wasm
/// address in `reg` and `len` sits in `B`. Both are u32, so the sum is at most
/// 2^33-2 and the 64-bit addition cannot wrap — the check is exact rather than
/// merely conservative.
///
/// A zero length still traps when the start is past the end, which is what the
/// specification says and what the interpreter does.
```

## L1973 · `fn callee_shape<'a>(`

```
/// Signature of a callee, refused unless it fits the slice we can generate.
```

## L1988-1996 · `fn load_args(f: &mut Ctx, params: &[ValType], places: &[Place], n_stack: usize) -> i32 {`

```
/// Arguments sit on the operand stack, deepest first. They go straight from
/// their frame slots into the argument registers — the destinations are
/// distinct and the sources are memory, so the order is free.
///
/// Returns the bytes reserved below `rsp` for the arguments that did not fit
/// in registers; the caller gives them back after the call. Moving `rsp` per
/// call rather than reserving an outgoing area in the frame keeps every slot
/// displacement known at emit time — a single pass has no chance to revisit
/// them, and calls with more than five arguments are rare.
```

## L2002-2004 · `if bytes > 0 {`

```
// The stack arguments go down first, while `rax` is still free; the
// register ones follow, because loading them is what finally commits the
// argument registers.
```

## L2014-2016 · `f.asm.load32(A, Reg::Rbp, off);`

```
// The 32-bit load zero-extends, so the whole eight-byte slot
// is defined and a native callee sees no rubbish above the
// value.
```

## L2029-2031 · `match f.stack[base + i].loc {`

```
// `spill_arg_conflicts` hat vorher alles weggeraeumt, was ein
// direkter Zug nicht kann. Was hier noch in einem Register
// steht, darf ohne Umweg dorthin, wo es hin soll.
```

## L2033 · `Loc::Gpr(r) if r == dst => {} // sitzt schon richtig`

```
// sitzt schon richtig
```

## L2045-2046 · `let src = f.local(idx);`

```
// Direkt aus dem Variablenschlitz; der Umweg ueber den
// Stapelschlitz waere eine Kopie ohne Zweck.
```

## L2071-2075 · `fn after_call(f: &mut Ctx, n_args: usize, results: &[ValType], arg_bytes: i32) {`

```
/// After a call there is nothing to repair. The instance reserves its address
/// space once and only ever makes more of it readable, so **the memory base
/// never moves** — not across a call, not across `memory.grow`. That falls out
/// of the guard-page design rather than being an extra promise, and it is what
/// lets the base stay pinned without a reload per call.
```

## L2092-2094 · `let ti = *f`

```
// Before anything else. Nothing in a register survives a call — and a
// spill materialises through `r11`, which the indirect path below is about
// to hold the call target in.
```

## L2103-2110 · `let (places, n_stack) = arg_places(params);`

```
// Nothing in a register survives a call, also muss alles UNTER den
// Argumenten in die Schlitze. Die Argumente selbst nicht — `load_args`
// zieht sie direkt in die Argumentregister, statt sie erst wegzuschreiben
// und sofort wieder zu holen.
// EINMAL rechnen. `arg_places` legt ein Vec an, und zweimal je
// Aufrufstelle waren am Geraet gemessen +22 % Allokationen beim
// Uebersetzen (314 214 -> 383 471) — ohne dass irgendjemand etwas davon
// hat.
```

## L2119-2121 · `f.asm.mov_rr64(Reg::Rdi, VMCTX);`

```
// An import is a native function: it expects the context as its first
// argument the ordinary way. A wasm callee reads it out of the pinned
// register instead, so nothing has to be set up for one.
```

## L2123-2124 · `f.asm.store64(VMCTX, vmctx::FUEL, FUEL);`

```
// A native function may want to read the counter, or charge against
// it for work of its own, so it is handed over and taken back.
```

## L2141-2144 · `fn call_indirect(f: &mut Ctx, type_index: u32) -> Result<(), &'static str> {`

```
/// The one place a bounds check really is needed: a table index cannot be
/// covered by a guard page. The signature check compares CANONICAL ids, not
/// type indices — wasm types are structural, and comparing indices would
/// reject calls the spec allows.
```

## L2150-2154 · `let (places, n_stack) = arg_places(params); // einmal, siehe call_direct`

```
// Wie im direkten Pfad, und aus demselben Grund noch strenger: das
// Sprungziel landet weiter unten in `r11`, und genau dort materialisiert
// ein Spill. ALLES Wegschreiben muss deshalb VOR dieser Stelle passieren —
// der Tabellenindex liegt ueber den Argumenten, also erst er, dann die
// Konflikte, dann faellt bis zum Aufruf kein Spill mehr an.
```

## L2155 · `let (places, n_stack) = arg_places(params); // einmal, siehe call_direct`

```
// einmal, siehe `call_direct`
```

## L2157 · `f.pop_to(B); // table index, zero-extended by the 32-bit move`

```
// table index, zero-extended by the 32-bit move
```

## L2169-2170 · `f.asm.load64(T, VMCTX, vmctx::TABLE);`

```
// The target must be in hand before the arguments are loaded: loading
// them writes over `rcx` and `rdx`.
```

## L2180-2183 · `fn mem_disp(`

```
/// Turn a memory immediate into a displacement, folding an oversized offset
/// into the address register. `disp32` is SIGNED, so an offset above 2 GiB
/// cannot be encoded — and since both the address and the offset are u32, the
/// sum still fits inside the 8 GiB reservation, so folding is safe.
```

## L2201 · `Err("offset>4G")`

```
// Only reachable with memory64, which `features()` does not admit.
```

## L2206-2208 · `fn load(`

```
/// `addr = pop; push(mem[addr + offset])`. `pop_to` is a 32-bit move, so the
/// address register already holds the zero-extended wasm address — exactly
/// what the reservation's arithmetic needs.
```

## L2217-2218 · `let d = f.alloc_gpr();`

```
// Straight into a cache register: a loaded value is almost always used
// right away, and a move in between would be pure loss.
```

## L2225-2226 · `fn store(`

```
/// `value = pop; addr = pop; mem[addr + offset] = value` — wasm pushes the
/// address first, so the value comes off the stack first.
```

## L2239 · `enum Sign {`

```
/// Which way `fsign` bends the sign bit.
```

## L2246-2247 · `enum FCmp {`

```
/// How a float comparison maps onto `ucomis*`, whose unordered case sets
/// CF, ZF and PF all at once.
```

## L2257 · `fn sign_mask(fw: Fw) -> i64 {`

```
/// The sign-bit mask for a width, as an immediate.
```

## L2280 · `let d = f.fpop_to_pool();`

```
// The left operand is worked on where it lies and stays there.
```

## L2292-2294 · `fn fsign(f: &mut Ctx, fw: Fw, kind: Sign) {`

```
/// `abs`, `neg` and `copysign` are bit operations, not arithmetic — so a NaN
/// operand comes out with its payload intact, which is what wasm asks for and
/// what an arithmetic lowering would quietly destroy.
```

## L2303 · `f.asm.fandn(fw, FB, FA);`

```
// Clear the sign: keep everything the mask does NOT cover.
```

## L2314 · `f.fpop_to(FB); // the sign donor`

```
// the sign donor
```

## L2315 · `f.fpop_to(FA); // the magnitude`

```
// the magnitude
```

## L2318 · `f.asm.fand(fw, FB, FC); // just the donor's sign`

```
// just the donor's sign
```

## L2319 · `f.asm.fandn(fw, FC, FA); // the magnitude without its own sign`

```
// the magnitude without its own sign
```

## L2327-2331 · `fn fminmax(f: &mut Ctx, fw: Fw, is_min: bool) {`

```
/// wasm's `min`/`max` differ from the hardware's in two places, and both
/// matter: with a NaN operand wasm wants a NaN but `minss` returns its second
/// source, and `min(+0,-0)` must be `-0` while `minss` again just returns the
/// second source. So the three cases are separated by hand — unordered,
/// equal, and the ordinary case where the hardware instruction is right.
```

## L2346-2347 · `f.asm.bind(equal);`

```
// Equal, so the only question left is the sign of a zero. Or of the two
// sign bits gives -0, and of them gives +0 — exactly min and max.
```

## L2356-2357 · `f.asm.bind(nan);`

```
// Unordered: adding propagates the NaN and quiets it. wasm leaves the
// payload to the implementation, so any NaN will do.
```

## L2366-2369 · `fn fcmp(f: &mut Ctx, fw: Fw, kind: FCmp) {`

```
/// `ucomis*` sets CF, ZF and PF together, and unordered sets all three. So
/// `setb` would call a NaN "less than", which wasm forbids: every ordered
/// comparison has to be phrased with `seta`/`setae`, which need CF clear.
/// The two operands are swapped where that phrasing requires it.
```

## L2378 · `f.asm.and32(A, B); // equal AND ordered`

```
// equal AND ordered
```

## L2384 · `f.asm.or32(A, B); // different OR unordered — ne is the one`

```
// different OR unordered — `ne` is the one
```

## L2385 · `}`

```
// comparison that is TRUE for a NaN
```

## L2413-2416 · `fn u64_to_f(f: &mut Ctx, fw: Fw) {`

```
/// u64 to float. x86 only converts SIGNED integers, so a value with the top
/// bit set has to be halved first, converted, and doubled back. Halving with
/// a plain shift would throw away the lowest bit and round wrong, so the lost
/// bit is folded back in — round-to-odd — before the conversion.
```

## L2437-2442 · `fn trunc_sat_s(f: &mut Ctx, fw: Fw, w: bool) {`

```
/// Signed saturating truncation. `cvtt*2si` answers with the "integer
/// indefinite" value — the minimum — for a NaN, for either overflow, AND for
/// a legitimate minimum. wasm wants 0 for the NaN, the maximum for a positive
/// overflow, and the minimum for the other two, so the ambiguous answer has
/// to be taken apart afterwards. It is the rare path, so it sits behind the
/// branch rather than in front of it.
```

## L2452-2455 · `if w {`

```
// The indefinite value is the width's own minimum, and in 64 bits that
// does not fit an immediate — a sign-extended `imm32` would compare
// against 0xFFFFFFFF80000000 instead, and every conversion would look
// ordinary.
```

## L2464 · `f.asm.xor(false, A, A); // a NaN answers zero`

```
// a NaN answers zero
```

## L2469 · `f.asm.fxor(fw, FB, FB); // 0.0`

```
// 0.0
```

## L2480-2483 · `fn trunc_sat_u32(f: &mut Ctx, fw: Fw) {`

```
/// Unsigned saturating truncation to i32. There is no unsigned convert, but
/// the whole u32 range fits comfortably inside a signed i64 — so the range is
/// fenced off first and the conversion then runs in 64 bits, where it cannot
/// go indefinite.
```

## L2485 · `let two32 = if fw.is_double() { 0x41F0_0000_0000_0000u64 } else { 0x4F80_0000 };`

```
// 2^32, as the bits of the respective float.
```

## L2488 · `f.asm.xor(false, A, A); // NaN and everything at or below zero answer 0`

```
// NaN and everything at or below zero answer 0
```

## L2498 · `f.asm.mov_r64_imm64(A, 0xFFFF_FFFF); // saturates; mov leaves flags alone`

```
// saturates; `mov` leaves flags alone
```

## L2508-2510 · `#[derive(Copy, Clone)]`

```
/// One ALU operation in its three encodings: against a register, against
/// memory, and against an immediate. Having all three lets the right-hand
/// operand be used where it already is.
```

## L2525 · `enum Rhs {`

```
/// Where the right-hand operand can be reached without moving it.
```

## L2527 · `Imm(i32),`

```
/// Fold it into the instruction as an immediate.
```

## L2529 · `Mem(i32),`

```
/// Address it in place — a local's slot or a spilled stack slot.
```

## L2531 · `Reg,`

```
/// Nothing clever available; it went into `rcx`.
```

## L2535-2539 · `fn take_rhs(f: &mut Ctx) -> Rhs {`

```
/// Take the right-hand operand off the stack WITHOUT materialising it, if it
/// is somewhere an instruction can reach directly. This is where most of the
/// remaining memory traffic goes away: `LocalGet` is 23,4 % of all
/// instructions and `I32Const` 17 %, and as a right-hand operand neither of
/// them needs a register at all.
```

## L2564-2565 · `fn bin(f: &mut Ctx, w: bool, op: AluOp) {`

```
/// `b = pop; a = pop; push(a OP b)` — wasm's operand order, so the first
/// popped value is the right-hand side and the direction of `sub` is kept.
```

## L2568-2569 · `let d = f.pop_to_pool();`

```
// The left-hand operand is worked on where it lies, and the result stays
// there. No move in, no move out.
```

## L2590-2591 · `fn shift(f: &mut Ctx, w: bool, ext: u8) {`

```
/// x86 takes a variable shift count from `cl` and nowhere else — but a
/// constant count is an immediate, and most counts in real code are constant.
```

## L2611 · `fn cmp(f: &mut Ctx, w: bool, cc: Cond) {`

```
/// A comparison consumes operands of its own width and always leaves an i32.
```

## L2624-2627 · `fn div_op(f: &mut Ctx, w: bool, signed: bool, rem: bool) {`

```
/// Divide or take the remainder. The dividend has to be in the accumulator
/// and the high half has to be prepared, so the operand registers are not
/// free here the way they are elsewhere: `rax` takes the dividend, `rdx` is
/// clobbered as the high half, and the divisor goes in `rcx`.
```

## L2629 · `f.pop_to(B); // divisor`

```
// divisor
```

## L2630 · `f.pop_to(A); // dividend`

```
// dividend
```

## L2634-2636 · `f.asm.cmp_r_imm32(w, B, -1);`

```
// A divisor of -1 leaves remainder 0 for every dividend, INT_MIN
// included — and that is the one case where wasm wants an answer
// rather than a trap.
```

## L2651-2652 · `f.asm.xor(false, C, C);`

```
// The high half must be zero, or `div` would work from rubbish
// and could even overflow.
```

## L2670-2674 · `fn clz(f: &mut Ctx, w: bool) {`

```
/// `clz`: `bsr` gives the index of the highest set bit, so `width-1 - index`
/// is the answer — and `xor` with `width-1` computes that. The sentinel for a
/// zero input is picked so the SAME `xor` turns it into `width`:
/// `63 ^ 31 = 32`, `127 ^ 63 = 64`. `mov` and `cmov` leave flags alone, so
/// ZF from `bsr` still holds when the `cmov` reads it.
```

## L2686-2687 · `fn ctz(f: &mut Ctx, w: bool) {`

```
/// `ctz`: `bsf` is already the answer for a non-zero input; zero takes the
/// full width.
```

