# `kernel/src/smp/scheduler.rs` @ 5e0102684

## L1-13 · `use alloc::collections::VecDeque;`

```
//! Task inbox: where new work waits until a worker core takes it.
//!
//! **One multi-producer queue, not a per-core Chase-Lev deque.** The deque
//! allowed exactly one producer — its owner core — and `spawn` pushed into
//! `DEQUES[0]` from wherever it was called. Apps spawn from worker cores
//! (`npk_spawn_module`, `npk_open`, `npk_launch`, `npk_pick`, the microVM AP
//! fallback), so two concurrent spawns could write the same slot: one task
//! lost, the other run twice. The per-core deques were never used otherwise
//! (`spawn_local` had no caller) and cost 256 × 256 slots of static memory.
//!
//! Workers take from the inbox in FIFO order. This is the interim form of the
//! per-core mailbox in `docs/plan/CORES_AND_EVENTS.md` §3.3; waking an idle
//! core with an IPI comes with the deadline timer (stage 1/2).
```

## L19 · `pub struct Task {`

```
// ── Task ───────────────────────────────────────────────────────
```

## L21-23 · `pub struct Task {`

```
/// A unit of new work. There is no priority: nothing ever read it, and
/// ordering among runnable work belongs to the per-core fiber scheduler
/// (`docs/plan/CORES_AND_EVENTS.md`), not to the inbox.
```

## L25-26 · `pub name: &'static str,`

```
/// What it is, for `cores` — a native task holds its core until it
/// returns, and a name is the first question when one does not.
```

## L30-32 · `pub is_fiber: bool,`

```
/// Run this task as a stackful FIBER on the worker core (its `func`
/// may block at yield points). Native run-to-completion tasks
/// (intents) leave this false and run directly. See `smp::fiber`.
```

## L36 · `static INBOX: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());`

```
// ── Global Scheduler State ─────────────────────────────────────
```

## L38-39 · `static INBOX: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());`

```
/// New work, from any core. Never touched from interrupt context, so the
/// spin lock needs no IRQ masking.
```

## L42 · `static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);`

```
/// Number of active worker cores (excludes BSP)
```

## L45 · `static TASKS_SPAWNED: AtomicU64 = AtomicU64::new(0);`

```
/// Total tasks spawned (monotonic counter)
```

## L47 · `static TASKS_TAKEN: AtomicU64 = AtomicU64::new(0);`

```
/// Total tasks taken by a worker
```

## L50 · `pub fn worker_count() -> usize {`

```
/// Number of worker cores (excludes the BSP).
```

## L59 · `pub fn spawn(name: &'static str, func: fn(u64), arg: u64) {`

```
// ── Public API ─────────────────────────────────────────────────
```

## L61 · `pub fn spawn(name: &'static str, func: fn(u64), arg: u64) {`

```
/// Queue a native run-to-completion task (an intent). Callable from any core.
```

## L66-68 · `pub fn spawn_fiber(func: fn(u64), arg: u64) {`

```
/// Like `spawn`, but the task runs as a stackful FIBER on the worker core
/// (`smp::fiber`). Used for `wasm_worker_task` so apps run on their own
/// stack and yield at `npk_sleep` instead of pinning the core.
```

## L80 · `super::per_core::wake_idle_workers();`

```
// Idle workers have no periodic tick any more — wake them.
```

## L84 · `pub fn has_work() -> bool {`

```
/// Is anything waiting in the inbox? For the idle re-check.
```

## L89 · `pub fn next_task(_core_id: usize) -> Option<Task> {`

```
/// Take the oldest waiting task, if any.
```

## L98-100 · `pub fn stats() -> (u64, u64, u64, usize) {`

```
/// Scheduler stats: (spawned, taken, steals, workers). `steals` is always 0
/// since there is nothing left to steal from; the slot stays for the
/// `npk_sys_info` ABI.
```

## L110-111 · `pub fn queue_len(core_id: usize) -> usize {`

```
/// Tasks waiting to be taken. The inbox is global; it is reported on core 0,
/// where the old per-core view showed all pending work as well.
```

