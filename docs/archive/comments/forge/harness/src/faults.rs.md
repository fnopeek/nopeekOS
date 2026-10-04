# `forge/harness/src/faults.rs` @ 5e0102684

## L1-14 · `use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};`

```
//! Turning a processor fault into a wasm trap.
//!
//! Two of the traps the generator relies on are not raised by an instruction
//! it emits: an access past the end of linear memory arrives as a page fault,
//! and a bad division as #DE. Both are the CPU telling us something a check
//! would otherwise have had to look for on every single operation — which is
//! exactly the trade the guard page and the bare `idiv` were chosen for.
//!
//! Catching them means pointing the interrupted context at the module's trap
//! routine with the reason in `rax`. Everything after that is the routine's
//! ordinary work. **The kernel's #PF and #DE handlers will do precisely this**,
//! against the same trap routine and the same instance context — the only
//! difference is that they read the interrupted registers out of a trap frame
//! rather than a `ucontext`.
```

## L18-19 · `static BASE: AtomicU64 = AtomicU64::new(0);`

```
/// The code region currently able to fault, and where its traps go. One
/// instance runs at a time here, so three words are enough.
```

## L25-26 · `fn ours(rip: u64) -> bool {`

```
/// Was the fault inside generated code? That is the whole test — a fault
/// anywhere else is a real one and must stay real.
```

## L34 · `unsafe {`

```
// SAFETY: the third argument of a `SA_SIGINFO` handler is a `ucontext_t`.
```

## L39-40 · `libc::signal(sig as libc::c_int, libc::SIG_DFL);`

```
// Not ours. Put the default back and return, so the fault happens
// again and ends the process the way it should.
```

## L44-47 · `let entry = if sig == libc::SIGFPE {`

```
// Point the interrupted context at the entry for this fault. The
// entry names the reason itself, so no general register has to be
// touched — which is what lets the kernel do the same with almost
// nothing.
```

## L57 · `pub fn install() {`

```
/// Install the handlers once.
```

## L62-63 · `unsafe {`

```
// SAFETY: a plain `sigaction` for two signals, with a handler that
// only reads and writes the interrupted context.
```

## L76 · `pub fn arm(base: u64, len: usize, pf: u64, de: u64) {`

```
/// Say which code may fault, and where its traps go.
```

