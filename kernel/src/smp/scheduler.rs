//! Work-Stealing Scheduler
//!
//! Each core has a local task deque. Cores push tasks locally and
//! steal from random peers when idle. MONITOR/MWAIT for zero-cost
//! sleep when no work is available.
//!
//! Design: no hardcoded core limit, no global run queue lock.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

// ── Task ───────────────────────────────────────────────────────

/// Task priority (higher = more urgent, scheduled first)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Priority {
    /// Background work (GC, prefetch, analytics)
    Background = 0,
    /// Normal tasks (file I/O, network, WASM modules)
    Normal = 1,
    /// Interactive tasks (compositor render, input handling)
    Interactive = 2,
    /// Critical tasks (IRQ bottom-half, timer callbacks)
    Critical = 3,
}

/// A unit of work that can be dispatched to any core
pub struct Task {
    pub id: u64,
    pub priority: Priority,
    /// Function to execute. Takes a u64 argument (context/data pointer).
    pub func: fn(u64),
    pub arg: u64,
}

/// Global task ID counter
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_task_id() -> u64 {
    NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Per-Core Deque ─────────────────────────────────────────────
//
// Chase-Lev work-stealing deque (simplified):
// - Owner pushes/pops from the TAIL (LIFO for locality)
// - Thieves steal from the HEAD (FIFO, oldest tasks first)
// - Array-based, fixed capacity (no resize needed for OS scheduler)

const DEQUE_CAPACITY: usize = 256;

/// Per-core work-stealing deque
pub struct WorkDeque {
    /// Circular buffer of tasks (only id + priority + func + arg stored inline)
    buffer: [TaskSlot; DEQUE_CAPACITY],
    /// Tail index — only written by owner core
    tail: AtomicUsize,
    /// Head index — read/CAS by thieves, read by owner
    head: AtomicUsize,
    /// Flag for MONITOR/MWAIT: non-zero when tasks are available
    pub wake_flag: AtomicU32,
}

#[derive(Clone, Copy)]
struct TaskSlot {
    id: u64,
    priority: u8,
    func: Option<fn(u64)>,
    arg: u64,
}

impl TaskSlot {
    const EMPTY: Self = TaskSlot { id: 0, priority: 0, func: None, arg: 0 };
}

impl WorkDeque {
    pub const fn new() -> Self {
        WorkDeque {
            buffer: [TaskSlot::EMPTY; DEQUE_CAPACITY],
            tail: AtomicUsize::new(0),
            head: AtomicUsize::new(0),
            wake_flag: AtomicU32::new(0),
        }
    }

    /// Push a task (called by owner core only). Returns false if full.
    pub fn push(&mut self, task: Task) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail.wrapping_sub(head) >= DEQUE_CAPACITY {
            return false; // Full
        }

        let idx = tail % DEQUE_CAPACITY;
        self.buffer[idx] = TaskSlot {
            id: task.id,
            priority: task.priority as u8,
            func: Some(task.func),
            arg: task.arg,
        };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        // Signal any MWAIT-sleeping core watching this deque
        self.wake_flag.store(1, Ordering::Release);
        true
    }

    /// Pop a task from tail (called by owner core only, LIFO).
    pub fn pop(&mut self) -> Option<Task> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == 0 {
            return None;
        }
        let new_tail = tail.wrapping_sub(1);
        self.tail.store(new_tail, Ordering::Relaxed);

        // Fence to ensure tail write is visible before we read head
        core::sync::atomic::fence(Ordering::SeqCst);

        let head = self.head.load(Ordering::Relaxed);
        if new_tail > head {
            // No contention — we got the slot
            let idx = new_tail % DEQUE_CAPACITY;
            let slot = self.buffer[idx];
            slot.func.map(|f| Task {
                id: slot.id,
                priority: unsafe { core::mem::transmute(slot.priority) },
                func: f,
                arg: slot.arg,
            })
        } else if new_tail == head {
            // One item left — race with thieves, use CAS
            let got = self.head.compare_exchange(
                head, head.wrapping_add(1),
                Ordering::SeqCst, Ordering::Relaxed,
            ).is_ok();
            // Reset tail regardless (empty now)
            self.tail.store(head.wrapping_add(1), Ordering::Relaxed);
            if got {
                let idx = new_tail % DEQUE_CAPACITY;
                let slot = self.buffer[idx];
                slot.func.map(|f| Task {
                    id: slot.id,
                    priority: unsafe { core::mem::transmute(slot.priority) },
                    func: f,
                    arg: slot.arg,
                })
            } else {
                None
            }
        } else {
            // Underflow — thieves took everything
            self.tail.store(head, Ordering::Relaxed);
            None
        }
    }

    /// Steal a task from head (called by thief cores, FIFO).
    pub fn steal(&self) -> Option<Task> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);

        if head >= tail {
            return None; // Empty
        }

        let idx = head % DEQUE_CAPACITY;
        let slot = self.buffer[idx];

        // CAS the head forward — if another thief beat us, retry is caller's job
        if self.head.compare_exchange(
            head, head.wrapping_add(1),
            Ordering::SeqCst, Ordering::Relaxed,
        ).is_ok() {
            slot.func.map(|f| Task {
                id: slot.id,
                priority: unsafe { core::mem::transmute(slot.priority) },
                func: f,
                arg: slot.arg,
            })
        } else {
            None // Lost race — caller can retry
        }
    }

    /// Number of tasks in the deque
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// SAFETY: WorkDeque uses atomics for shared state, buffer is only
// written by owner (push/pop) and read by thieves (steal).
unsafe impl Sync for WorkDeque {}
unsafe impl Send for WorkDeque {}

// ── Global Scheduler State ─────────────────────────────────────

/// Maximum supported cores (array size, not a hard limit on discovery)
const MAX_CORES: usize = 256;

/// Per-core deques. Index = core_id (0 = BSP, 1..N = APs).
static mut DEQUES: [WorkDeque; MAX_CORES] = {
    const EMPTY: WorkDeque = WorkDeque::new();
    [EMPTY; MAX_CORES]
};

/// Number of active worker cores (excludes BSP)
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Simple PRNG for random victim selection (per-steal, not crypto)
static STEAL_RNG: AtomicU64 = AtomicU64::new(0x5851_F42D_4C95_7F2D);

fn random_core(max_exclusive: usize) -> usize {
    // xorshift64
    let mut s = STEAL_RNG.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    STEAL_RNG.store(s, Ordering::Relaxed);
    (s as usize) % max_exclusive
}

/// Initialize scheduler (called from smp::init after APs are booted)
pub fn init(num_workers: usize) {
    WORKER_COUNT.store(num_workers, Ordering::Release);
    // Seed RNG from TSC
    STEAL_RNG.store(crate::interrupts::rdtsc(), Ordering::Relaxed);
}

// ── Public API ─────────────────────────────────────────────────

/// Spawn a task. Dispatches to the least-loaded worker core.
/// Called from BSP (Core 0) or any core.
pub fn spawn(priority: Priority, func: fn(u64), arg: u64) {
    let workers = WORKER_COUNT.load(Ordering::Acquire);
    if workers == 0 {
        // No workers — run inline on BSP
        func(arg);
        return;
    }

    let task = Task {
        id: alloc_task_id(),
        priority,
        func,
        arg,
    };

    // Find least-loaded worker (cores 1..=workers)
    let mut best_core = 1usize;
    let mut best_len = usize::MAX;
    for i in 1..=workers {
        // SAFETY: index is within bounds (i <= workers < MAX_CORES)
        let len = unsafe { DEQUES[i].len() };
        if len < best_len {
            best_len = len;
            best_core = i;
        }
    }

    // Push to target core's deque
    // SAFETY: we're the only writer for this core's deque from BSP side,
    // or the push is from the owner core itself.
    let pushed = unsafe { DEQUES[best_core].push(task) };
    if !pushed {
        // Queue full — run inline as fallback
        func(arg);
    }
}

/// Called by AP in its run loop: try own deque, then steal.
/// Returns None if no work found anywhere.
pub fn next_task(core_id: usize) -> Option<Task> {
    // Try own deque first (LIFO — cache locality)
    // SAFETY: core_id is this core's own index, we're the owner
    let task = unsafe { DEQUES[core_id].pop() };
    if task.is_some() {
        return task;
    }

    // Try stealing from a random peer
    let workers = WORKER_COUNT.load(Ordering::Relaxed);
    if workers <= 1 {
        return None;
    }

    // Try up to `workers` random victims
    let total_cores = workers + 1; // include BSP's deque (index 0)
    for _ in 0..workers {
        let victim = random_core(total_cores);
        if victim == core_id {
            continue;
        }
        // SAFETY: victim index is within bounds
        let stolen = unsafe { DEQUES[victim].steal() };
        if stolen.is_some() {
            return stolen;
        }
    }

    None
}

/// Get wake_flag address for a core's deque (for MONITOR instruction)
pub fn wake_flag_ptr(core_id: usize) -> *const AtomicU32 {
    // SAFETY: core_id within bounds
    unsafe { &DEQUES[core_id].wake_flag as *const AtomicU32 }
}

/// Clear wake flag (called by AP after waking from MWAIT)
pub fn clear_wake(core_id: usize) {
    // SAFETY: core_id within bounds
    unsafe { DEQUES[core_id].wake_flag.store(0, Ordering::Relaxed); }
}
