//! Work-Stealing Scheduler
//!
//! Chase-Lev deque per core (SPMC: Single Producer, Multiple Consumer).
//! Golden rule: only the OWNER core may push()/pop(). All others steal().
//!
//! Work distribution: BSP pushes to DEQUES[0], idle APs steal from there.
//! APs that spawn sub-tasks push to their own deque.
//! Global WORK_AVAILABLE flag wakes all sleeping APs via MONITOR/MWAIT.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

// ── Task ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Priority {
    Background = 0,
    Normal = 1,
    Interactive = 2,
    Critical = 3,
}

pub struct Task {
    pub id: u64,
    pub priority: Priority,
    pub func: fn(u64),
    pub arg: u64,
}

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_task_id() -> u64 {
    NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Per-Core Deque (Chase-Lev) ─────────────────────────────────
//
// SPMC: Owner pushes/pops from TAIL, thieves steal from HEAD.
// push() and pop() are NOT thread-safe against each other from
// different cores — only the owner core may call them.

const DEQUE_CAPACITY: usize = 256;

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

pub struct WorkDeque {
    buffer: [TaskSlot; DEQUE_CAPACITY],
    tail: AtomicUsize,
    head: AtomicUsize,
}

impl WorkDeque {
    pub const fn new() -> Self {
        WorkDeque {
            buffer: [TaskSlot::EMPTY; DEQUE_CAPACITY],
            tail: AtomicUsize::new(0),
            head: AtomicUsize::new(0),
        }
    }

    /// Push a task. OWNER CORE ONLY — not thread-safe for multiple pushers.
    pub fn push(&mut self, task: Task) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail.wrapping_sub(head) >= DEQUE_CAPACITY {
            return false;
        }

        self.buffer[tail % DEQUE_CAPACITY] = TaskSlot {
            id: task.id,
            priority: task.priority as u8,
            func: Some(task.func),
            arg: task.arg,
        };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Pop from tail. OWNER CORE ONLY (LIFO — cache locality).
    pub fn pop(&mut self) -> Option<Task> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        if tail == head {
            return None; // Empty
        }

        let new_tail = tail.wrapping_sub(1);
        self.tail.store(new_tail, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::SeqCst);

        let head = self.head.load(Ordering::Relaxed);
        if new_tail > head {
            Some(self.slot_to_task(new_tail))
        } else if new_tail == head {
            // Last item — race with thieves
            if self.head.compare_exchange(
                head, head.wrapping_add(1),
                Ordering::SeqCst, Ordering::Relaxed,
            ).is_ok() {
                self.tail.store(head.wrapping_add(1), Ordering::Relaxed);
                Some(self.slot_to_task(new_tail))
            } else {
                self.tail.store(head.wrapping_add(1), Ordering::Relaxed);
                None
            }
        } else {
            self.tail.store(head, Ordering::Relaxed);
            None
        }
    }

    /// Steal from head. Safe to call from ANY core (FIFO — oldest first).
    pub fn steal(&self) -> Option<Task> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);

        if head >= tail {
            return None;
        }

        let slot = self.buffer[head % DEQUE_CAPACITY];

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
            None
        }
    }

    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    fn slot_to_task(&self, index: usize) -> Task {
        let slot = self.buffer[index % DEQUE_CAPACITY];
        Task {
            id: slot.id,
            priority: unsafe { core::mem::transmute(slot.priority) },
            func: slot.func.unwrap(),
            arg: slot.arg,
        }
    }
}

unsafe impl Sync for WorkDeque {}
unsafe impl Send for WorkDeque {}

// ── Global Scheduler State ─────────────────────────────────────

const MAX_CORES: usize = 256;

/// Per-core deques. Index = core_id. Only owner pushes/pops, others steal.
static mut DEQUES: [WorkDeque; MAX_CORES] = {
    const EMPTY: WorkDeque = WorkDeque::new();
    [EMPTY; MAX_CORES]
};

/// Number of active worker cores (excludes BSP)
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Total tasks spawned (monotonic counter)
static TASKS_SPAWNED: AtomicU64 = AtomicU64::new(0);
/// Total tasks completed
static TASKS_COMPLETED: AtomicU64 = AtomicU64::new(0);
/// Total steals performed
static STEALS: AtomicU64 = AtomicU64::new(0);

/// Global wake signal — ALL APs MONITOR this address.
/// Set to 1 when new work is available, cleared by APs after waking.
/// Aligned to cache line to avoid false sharing.
#[repr(align(64))]
pub struct WakeSignal {
    pub flag: AtomicU32,
}

pub static WORK_AVAILABLE: WakeSignal = WakeSignal {
    flag: AtomicU32::new(0),
};

/// Simple PRNG for random victim selection
static STEAL_RNG: AtomicU64 = AtomicU64::new(0x5851_F42D_4C95_7F2D);

fn random_core(max_exclusive: usize) -> usize {
    let mut s = STEAL_RNG.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    STEAL_RNG.store(s, Ordering::Relaxed);
    (s as usize) % max_exclusive
}

pub fn init(num_workers: usize) {
    WORKER_COUNT.store(num_workers, Ordering::Release);
    STEAL_RNG.store(crate::interrupts::rdtsc(), Ordering::Relaxed);
}

// ── Public API ─────────────────────────────────────────────────

/// Spawn a task from BSP (Core 0).
/// Pushes to BSP's own deque — idle APs will steal it automatically.
/// This is the ONLY correct way to add work from Core 0.
pub fn spawn(priority: Priority, func: fn(u64), arg: u64) {
    let workers = WORKER_COUNT.load(Ordering::Acquire);
    if workers == 0 {
        func(arg);
        return;
    }

    let task = Task {
        id: alloc_task_id(),
        priority,
        func,
        arg,
    };

    // Push to BSP's own deque (Core 0 is the owner → safe)
    // SAFETY: spawn() is called from BSP, DEQUES[0] owner is BSP
    let pushed = unsafe { DEQUES[0].push(task) };
    if pushed {
        TASKS_SPAWNED.fetch_add(1, Ordering::Relaxed);
        WORK_AVAILABLE.flag.store(1, Ordering::Release);
    } else {
        func(arg);
    }
}

/// Spawn a sub-task from a worker AP. Pushes to the calling core's OWN deque.
pub fn spawn_local(core_id: usize, priority: Priority, func: fn(u64), arg: u64) {
    let task = Task {
        id: alloc_task_id(),
        priority,
        func,
        arg,
    };

    // SAFETY: core_id is the caller's own core → owner, safe to push
    let pushed = unsafe { DEQUES[core_id].push(task) };
    if pushed {
        WORK_AVAILABLE.flag.store(1, Ordering::Release);
    } else {
        func(arg);
    }
}

/// Try to get work: own deque first (pop), then steal from others.
pub fn next_task(core_id: usize) -> Option<Task> {
    // Own deque first (LIFO — cache locality)
    // SAFETY: core_id is caller's own index
    let task = unsafe { DEQUES[core_id].pop() };
    if task.is_some() {
        return task;
    }

    // Steal from peers (try all cores, random start)
    let workers = WORKER_COUNT.load(Ordering::Relaxed);
    let total = workers + 1; // BSP + workers
    let start = random_core(total);
    for i in 0..total {
        let victim = (start + i) % total;
        if victim == core_id {
            continue;
        }
        // SAFETY: steal() is safe from any core (SPMC consumer side)
        let stolen = unsafe { DEQUES[victim].steal() };
        if stolen.is_some() {
            return stolen;
        }
    }

    None
}

/// Address of the global wake flag (for MONITOR instruction)
pub fn wake_flag_ptr() -> *const AtomicU32 {
    &WORK_AVAILABLE.flag as *const AtomicU32
}

/// Clear global wake flag (called by AP after waking)
pub fn clear_wake() {
    WORK_AVAILABLE.flag.store(0, Ordering::Relaxed);
}

/// Record a completed task (called after task.func returns)
pub fn mark_completed() {
    TASKS_COMPLETED.fetch_add(1, Ordering::Relaxed);
}

/// Record a successful steal
pub fn mark_stolen() {
    STEALS.fetch_add(1, Ordering::Relaxed);
}

/// Scheduler stats: (spawned, completed, steals, workers, queue_depths)
pub fn stats() -> (u64, u64, u64, usize) {
    (
        TASKS_SPAWNED.load(Ordering::Relaxed),
        TASKS_COMPLETED.load(Ordering::Relaxed),
        STEALS.load(Ordering::Relaxed),
        WORKER_COUNT.load(Ordering::Relaxed),
    )
}

/// Per-core queue depth (for top display)
pub fn queue_len(core_id: usize) -> usize {
    if core_id >= MAX_CORES { return 0; }
    unsafe { DEQUES[core_id].len() }
}
