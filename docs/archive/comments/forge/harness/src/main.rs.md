# `forge/harness/src/main.rs` @ 5e0102684

## L1-7 · `mod bench;`

```
//! Host harness for forge. Runs the same `no_std` code the kernel will run,
//! on the real modules, and reports what it found.
//!
//!   forge_harness <file.wasm>...              census per module
//!   forge_harness --roadmap <file.wasm>       how far are we, and what next
//!   forge_harness --selftest                  generated code vs wasmi
//!   forge_harness --run <wasm> [page w h n]   run it, and time it
```

## L20-23 · `if args.first().map(|a| a == "--oneshot").unwrap_or(false) {`

```
// Run one module and print "<result> <trap>". Used by `gentests` to
// produce the device check's expectations by measurement rather than by
// hand — and it runs in its own process, so a trap that is meant to fault
// cannot take the generator down with it.
```

## L133-140 · `fn report_roadmap(f: &str) {`

```
/// A function is translatable exactly when every opcode it uses is one the
/// generator emits. Partial credit does not exist — so the honest question is
/// not "how many opcodes are done" but "how many FUNCTIONS does the next
/// opcode unlock". Greedy answers it, and the answer is the work order.
///
/// Kept incremental: each function carries a count of the opcodes it still
/// misses, and an opcode's gain is the number of its functions sitting at
/// exactly one. Otherwise python's 9939 x 147 turns into minutes.
```

## L153-154 · `let t0 = std::time::Instant::now();`

```
// The headline comes from the generator, not from a list of opcode names:
// a function counts only when `compile()` actually produced code for it.
```

## L185 · `let funcs: Vec<(Vec<usize>, u32)> = p`

```
// funcs[i] = (opcode indices, instruction count)
```

## L193 · `let mut users: Vec<Vec<usize>> = vec![Vec::new(); all_ops.len()];`

```
// Inverted index: which functions use each opcode.
```

## L207 · `let mut missing: Vec<u32> = funcs`

```
// How many opcodes each function still misses.
```

## L231-233 · `let wasm_code: u64 = m`

```
// Code size is one of the paper's cost items, so it is worth reporting
// against the wasm the functions came from — not against the whole file,
// which is mostly data.
```

## L249-250 · `{`

```
// Auszaehlung: wohin gehen die Bytes? Die Frage ist, ob der Abstand zu
// Cranelift an der Registerhaltung ueber Blockgrenzen haengt.
```

## L284 · `let (mut best, mut best_gain, mut best_users) = (usize::MAX, -1i64, 0usize);`

```
// Gain of an opcode: functions that are missing only this one.
```

