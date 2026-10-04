# `forge/harness/src/bench.rs` @ 5e0102684

## L1-9 · `use crate::selftest::{Exec, Inst};`

```
//! Run a real module under forge and under wasmi, and time both.
//!
//! Coverage is not speed. Everything up to here proved the generated code
//! computes the same thing the interpreter does; this is where it says whether
//! it does so faster, on the module whose slowness started the whole thing.
//!
//! `beakbench.wasm` is the vehicle because it is beak's own engine with a
//! single import, and because its warm layout has a known fuel count — if that
//! count does not appear, the two sides are not doing the same work.
```

## L18-20 · `extern "C" fn h_bench_log(ctx: *const u64, ptr: u32, len: u32) {`

```
/// `bench_log(ptr, len)` — the module's one import. It reaches into linear
/// memory through the instance context, exactly as a kernel host function
/// would.
```

## L23-24 · `unsafe {`

```
// SAFETY: `ctx` is the instance context of the module doing the call, and
// the range is checked against the memory's own recorded size.
```

## L222 · `let same = w.parse.1 == f.parse.1 && w.cascade.1 == f.cascade.1 && w.layout.1 == f.layout.1;`

```
// The results must agree before any timing is worth reading.
```

