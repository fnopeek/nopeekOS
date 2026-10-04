# `forge/harness/src/pybench.rs` @ 5e0102684

## L1-7 · `use crate::selftest::{Exec, Inst};`

```
//! Run `python.wasm` under both engines and time it.
//!
//! A wasi program finishes by leaving through `proc_exit`, not by returning —
//! the interpreter can turn that into an error and unwind, generated code
//! cannot. So both runs happen in a forked child that reports its own time,
//! exit status and stdout through a pipe on the way out. Same exit path, same
//! measurement, and the outputs can be compared byte for byte.
```

## L31 · `fn in_child(body: impl FnOnce(i32)) -> Option<Run> {`

```
/// Fork, let the child do the work and report, collect what it sent.
```

## L34 · `if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {`

```
// SAFETY: `fds` is a two-element array, which is what `pipe` writes.
```

## L39 · `let pid = unsafe { libc::fork() };`

```
// SAFETY: the child only runs `body` and never returns from it.
```

## L42 · `unsafe { libc::close(rd) };`

```
// SAFETY: closing the read end this child does not use.
```

## L45-46 · `unsafe { libc::_exit(0) };`

```
// Only reached if the module returned instead of calling `proc_exit`.
// SAFETY: ends the child without unwinding.
```

## L49 · `unsafe { libc::close(wr) };`

```
// SAFETY: closing the write end the parent does not use.
```

## L54 · `let n = unsafe { libc::read(rd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };`

```
// SAFETY: reading into a local buffer from a pipe we own.
```

## L61 · `unsafe { libc::close(rd) };`

```
// SAFETY: our own descriptor, closed once.
```

## L64 · `unsafe { libc::waitpid(pid, &mut st, 0) };`

```
// SAFETY: waiting for the child we just forked.
```

## L88-89 · `let t = Instant::now();`

```
// Translation happens in the parent, so its cost is reported separately
// and does not land inside the run.
```

## L116 · `wasi_core::report(&ctx, 0);`

```
// Returned without `proc_exit` — report anyway.
```

