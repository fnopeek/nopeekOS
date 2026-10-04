# `forge/harness/src/bin/gentests.rs` @ 5e0102684

## L1-5 · `fn forge_harness_run(wasm: &[u8], arg: u32, fuel: i64) -> (u32, u32) {`

```
//! Emit the kernel's on-device test modules as Rust byte arrays.
//!
//! They are built here rather than in the kernel because assembling wasm needs
//! `wat`, which is a std crate — and because a module that is checked on the
//! host first is one fewer unknown when the device says no.
```

## L7 · `fn forge_harness_run(wasm: &[u8], arg: u32, fuel: i64) -> (u32, u32) {`

```
/// Run a module here the same way the kernel will, and report what came out.
```

## L82 · `let args: &[(&str, u32, i64)] = &[`

```
// (name, argument, fuel budget)
```

