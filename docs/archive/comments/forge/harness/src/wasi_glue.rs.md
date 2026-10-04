# `forge/harness/src/wasi_glue.rs` @ 5e0102684

## L1-4 · `#![allow(dead_code)]`

```
//! Two adapters over ONE implementation. Everything the guest can observe
//! lives in `wasi_core`; all that differs here is how each engine hands over
//! guest memory and the context. Any other arrangement would compare host
//! layers instead of compilers.
```

## L10-12 · `static mut WASI: *mut WasiCtx = std::ptr::null_mut();`

```
/// The context of the single instance a run has. One run, one instance, one
/// thread — so a pointer parked here is enough, and it saves threading a
/// closure environment through generated code that has none.
```

## L16 · `unsafe { WASI = ctx as *mut WasiCtx };`

```
// SAFETY: the harness runs one instance at a time on one thread.
```

## L20-21 · `unsafe fn parts<'a>(vm: *const u64) -> (&'a mut [u8], &'a mut WasiCtx) {`

```
/// Guest memory and context, from the instance context. Both are re-read on
/// every call because `memory.grow` may have moved the end since the last one.
```

## L31 · `fn pair<'a>(caller: &'a mut Caller<'_, WasiCtx>) -> (&'a mut [u8], &'a mut WasiCtx) {`

```
/// The same pair, the way the interpreter offers it.
```

## L42 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L48 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L54 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L60 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L66 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L72 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L78 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L84 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L90 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L96 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L102 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L108 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L114 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L120 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L126 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L132 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L138 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L144 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L150 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L156 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L162 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L168 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L174 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L180 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L186 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L192 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L198 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L204 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L210 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L216 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L222 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L228 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L234 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L240 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L246 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L252 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L258 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L264 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L270 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L276 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L282 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L288 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` is the instance context of the module doing the call.
```

## L293 · `pub fn forge_table() -> &'static [(&'static str, u64)] {`

```
/// Name to address, for the instance's host-function array.
```

## L295 · `static_table()`

```
// Built once; the addresses are of `extern "C"` items and never move.
```

