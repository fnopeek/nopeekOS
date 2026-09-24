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

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ── Task ───────────────────────────────────────────────────────

/// A unit of new work. There is no priority: nothing ever read it, and
/// ordering among runnable work belongs to the per-core fiber scheduler
/// (`docs/plan/CORES_AND_EVENTS.md`), not to the inbox.
pub struct Task {
    /// What it is, for `cores` — a native task holds its core until it
    /// returns, and a name is the first question when one does not.
    pub name: &'static str,
    pub func: fn(u64),
    pub arg: u64,
    /// Run this task as a stackful FIBER on the worker core (its `func`
    /// may block at yield points). Native run-to-completion tasks
    /// (intents) leave this false and run directly. See `smp::fiber`.
    pub is_fiber: bool,
}

// ── Global Scheduler State ─────────────────────────────────────

/// New work, from any core. Never touched from interrupt context, so the
/// spin lock needs no IRQ masking.
static INBOX: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

/// Number of active worker cores (excludes BSP)
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Total tasks spawned (monotonic counter)
static TASKS_SPAWNED: AtomicU64 = AtomicU64::new(0);
/// Total tasks taken by a worker
static TASKS_TAKEN: AtomicU64 = AtomicU64::new(0);

/// Number of worker cores (excludes the BSP).
pub fn worker_count() -> usize {
    WORKER_COUNT.load(Ordering::Relaxed)
}

pub fn init(num_workers: usize) {
    WORKER_COUNT.store(num_workers, Ordering::Release);
}

// ── Public API ─────────────────────────────────────────────────

/// Queue a native run-to-completion task (an intent). Callable from any core.
pub fn spawn(name: &'static str, func: fn(u64), arg: u64) {
    spawn_inner(name, func, arg, false);
}

/// Like `spawn`, but the task runs as a stackful FIBER on the worker core
/// (`smp::fiber`). Used for `wasm_worker_task` so apps run on their own
/// stack and yield at `npk_sleep` instead of pinning the core.
pub fn spawn_fiber(func: fn(u64), arg: u64) {
    spawn_inner("fiber", func, arg, true);
}

fn spawn_inner(name: &'static str, func: fn(u64), arg: u64, is_fiber: bool) {
    if WORKER_COUNT.load(Ordering::Acquire) == 0 {
        func(arg);
        return;
    }
    INBOX.lock().push_back(Task { name, func, arg, is_fiber });
    TASKS_SPAWNED.fetch_add(1, Ordering::Relaxed);
    // Idle workers have no periodic tick any more — wake them.
    super::per_core::wake_idle_workers();
}

/// Is anything waiting in the inbox? For the idle re-check.
pub fn has_work() -> bool {
    !INBOX.lock().is_empty()
}

/// Take the oldest waiting task, if any.
pub fn next_task(_core_id: usize) -> Option<Task> {
    let t = INBOX.lock().pop_front();
    if t.is_some() {
        TASKS_TAKEN.fetch_add(1, Ordering::Relaxed);
    }
    t
}

/// Scheduler stats: (spawned, taken, steals, workers). `steals` is always 0
/// since there is nothing left to steal from; the slot stays for the
/// `npk_sys_info` ABI.
pub fn stats() -> (u64, u64, u64, usize) {
    (
        TASKS_SPAWNED.load(Ordering::Relaxed),
        TASKS_TAKEN.load(Ordering::Relaxed),
        0,
        WORKER_COUNT.load(Ordering::Relaxed),
    )
}

/// Tasks waiting to be taken. The inbox is global; it is reported on core 0,
/// where the old per-core view showed all pending work as well.
pub fn queue_len(core_id: usize) -> usize {
    if core_id == 0 { INBOX.lock().len() } else { 0 }
}
