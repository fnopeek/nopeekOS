//! Task inbox: where new work waits until a worker core takes it.
//!
//! One multi-producer queue rather than single-producer per-core deques:
//! apps spawn from worker cores (`npk_spawn_module`, `npk_open`,
//! `npk_launch`, `npk_pick`, the microVM AP fallback), so any core may push.
//!
//! Workers take from the inbox in FIFO order. This is the interim form of the
//! per-core mailbox in `docs/plan/CORES_AND_EVENTS.md` §3.3.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ── Task ───────────────────────────────────────────────────────

/// A unit of new work. There is no priority: ordering among runnable work
/// belongs to the per-core fiber scheduler
/// (`docs/plan/CORES_AND_EVENTS.md`), not to the inbox.
pub struct Task {
    /// What it is, for `cores` — a native task holds its core until it
    /// returns, and a name is the first question when one does not.
    pub name: &'static str,
    pub func: fn(u64),
    pub arg: u64,
    /// Run this task as a stackful fiber on the worker core (its `func`
    /// may block at yield points). Native run-to-completion tasks
    /// (intents) leave this false and run directly. See `smp::fiber`.
    pub is_fiber: bool,
    /// A background app (autostart: dock, bar). These share one core
    /// (`per_core::app_core`) instead of taking an empty core each, which
    /// a vCPU could have used.
    pub shared: bool,
    /// TSC when it was queued: a task no core has taken for long is taken
    /// by any core that takes work at all (`per_core::accepts`).
    pub queued: u64,
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
    spawn_inner(name, func, arg, false, false);
}

/// Like `spawn`, but the task runs as a stackful fiber on the worker core
/// (`smp::fiber`). Used for `wasm_worker_task` so apps run on their own
/// stack and yield at `npk_sleep` instead of pinning the core.
pub fn spawn_fiber(func: fn(u64), arg: u64) {
    spawn_inner("fiber", func, arg, true, false);
}

/// Like `spawn_fiber`, for a background app that shares the app core.
pub fn spawn_fiber_shared(func: fn(u64), arg: u64) {
    spawn_inner("fiber", func, arg, true, true);
}

fn spawn_inner(name: &'static str, func: fn(u64), arg: u64, is_fiber: bool, shared: bool) {
    if WORKER_COUNT.load(Ordering::Acquire) == 0 {
        func(arg);
        return;
    }
    let queued = crate::interrupts::rdtsc();
    INBOX.lock().push_back(Task { name, func, arg, is_fiber, shared, queued });
    TASKS_SPAWNED.fetch_add(1, Ordering::Relaxed);
    // Idle workers have no periodic tick, so wake them.
    super::per_core::wake_idle_workers();
}

/// Is anything waiting in the inbox? For the idle re-check.
pub fn has_work() -> bool {
    !INBOX.lock().is_empty()
}

/// Whether the oldest waiting task is one `accept` would take, given whether
/// it is a shared background app and how many TSC cycles it has waited. For
/// the idle re-check.
pub fn head_acceptable(accept: impl Fn(bool, u64) -> bool) -> bool {
    let now = crate::interrupts::rdtsc();
    INBOX.lock().front().is_some_and(|t| accept(t.shared, now.saturating_sub(t.queued)))
}

/// Take the oldest waiting task if `accept` takes it. The decision is made
/// under the inbox lock, so the task decided on is the task taken.
pub fn next_task(accept: impl Fn(bool, u64) -> bool) -> Option<Task> {
    let now = crate::interrupts::rdtsc();
    let mut inbox = INBOX.lock();
    if !inbox.front().is_some_and(|t| accept(t.shared, now.saturating_sub(t.queued))) {
        return None;
    }
    let t = inbox.pop_front();
    drop(inbox);
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

/// Tasks waiting to be taken. The inbox is global; it is reported on core 0.
pub fn queue_len(core_id: usize) -> usize {
    if core_id == 0 { INBOX.lock().len() } else { 0 }
}
