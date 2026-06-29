//! Stackful fibers (green threads) for WASM apps.
//!
//! See `SCHEDULER_FIBERS.md`. A fiber is "a stack + a saved context that
//! runs until it yields". wasmi cannot be paused mid-`_start`, so we give
//! each app its own stack and switch the whole CPU context at the yield
//! point (`npk_sleep`). The same primitive will later host guest-vCPU
//! run-loops (multicore microvm) — it only swaps `rsp` + the callee-saved
//! registers, so it is agnostic to what runs on the stack.
//!
//! - **Stage 1:** the context-switch primitive + a boot self-test.
//! - **Stage 2a:** `wasm_worker_task` runs on a fiber (own stack).
//! - **Stage 2b:** `npk_sleep` PARKS the fiber + switches back to a per-core
//!   scheduler that round-robins the core's other fibers → many apps
//!   multiplex over few workers (no more core-pinning / helper-nesting).
//!
//! A fiber is pinned to the core that admitted it (its wasm `HostState`
//! caches `core_id`), so each core owns its queue — no cross-core hot path.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Saved execution context. Only `rsp` lives here — the callee-saved
/// registers (rbx, rbp, r12–r15) are pushed onto the fiber's own stack by
/// `switch` and popped back on resume, System V style.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    pub rsp: u64,
}

impl Context {
    pub const fn empty() -> Self {
        Context { rsp: 0 }
    }
}

// The switch + the fresh-fiber entry trampoline. AT&T syntax to match
// boot.s / trampoline.s. System V args: rdi = `from`, rsi = `to`.
//
//   switch: save callee-saved + rsp into *from, load *to's rsp + regs,
//           `ret` into to's resume point.
//   trampoline: where a *fresh* fiber's first `ret` lands. The initial
//           frame put the entry fn in r12 and its arg in r13 (they were
//           just popped by `switch`), so move the arg into rdi and call
//           the entry. If the entry ever returns, fall into `fiber_on_exit`
//           (Stage 2 hooks the switch-back-to-scheduler there).
core::arch::global_asm!(
    r#"
.global fiber_context_switch
fiber_context_switch:
    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, (%rdi)
    movq (%rsi), %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    ret

.global fiber_trampoline
fiber_trampoline:
    movq %r13, %rdi
    callq *%r12
    callq fiber_on_exit
.Lfiber_hang:
    hlt
    jmp .Lfiber_hang
"#,
    options(att_syntax)
);

unsafe extern "C" {
    fn fiber_context_switch(from: *mut Context, to: *const Context);
    fn fiber_trampoline();
}

/// Called by the trampoline when a fiber's entry function returns — the
/// app's `_start` ran to completion. Mark the fiber Done and switch back to
/// the owning core's scheduler context, which then frees the stack. Never
/// returns to the trampoline.
#[unsafe(no_mangle)]
pub extern "C" fn fiber_on_exit() {
    let cid = crate::smp::per_core::current_core_id();
    // SAFETY: CURRENT_FIBER[cid] is the finishing fiber (set by the
    // scheduler before switching in); SCHED_CTX[cid] is the core's loop ctx.
    unsafe {
        let f = CURRENT_FIBER[cid];
        if !f.is_null() {
            (*f).state = FiberState::Done;
            switch(&raw mut (*f).ctx, &raw const SCHED_CTX[cid]);
        }
    }
    // Unreachable: control resumes in `run_core_fibers` after its `switch`.
    // (If CURRENT_FIBER was somehow null, fall through to the trampoline's
    // own hlt loop.)
}

/// Default per-fiber stack size. 128 KiB — comfortable headroom over the
/// 64 KiB AP stacks apps already run wasmi on today (and the old nesting
/// stacked two wasmi instances on those 64 KiB). The WASM linear memory is
/// separate (on the heap), so this only holds the interpreter + host-fn
/// call frames. No guard page yet (heap-backed); overflow = corruption.
pub const DEFAULT_STACK_BYTES: usize = 128 * 1024;

// ── Per-core fiber scheduler (Stage 2b) ────────────────────────────────
//
// Each app is a fiber pinned to the core that admitted it. `npk_sleep`
// parks the running fiber (Sleeping + a TSC wake-deadline) and switches
// back to the core's scheduler context, FREEING the core to run its other
// ready fibers. So dock+bar+loft+spell multiplex over a couple of workers
// instead of pinning a core each or nesting. No fiber migrates between
// cores, so each core owns its queue with no cross-core hot path.

const MAX_CORES: usize = 256;

#[derive(Clone, Copy, PartialEq)]
enum FiberState {
    Ready,
    Sleeping(u64), // resume once rdtsc() >= deadline
    /// Parked on a device IRQ (`crate::irq`): resume once the vector's
    /// fired-count moves past `since`, or `deadline` (timeout) passes. The
    /// IRQ wakes this core out of HLT, so the scheduler re-runs and resumes
    /// here with ~no latency — no polling.
    WaitingIrq { vector: u8, since: u64, deadline: u64 },
    /// Parked until this core's net-kick generation moves past `gen` (an
    /// off-vCPU producer injected + kicked us), or `deadline` (timeout) passes.
    /// The kick's IPI wakes this core out of HLT, so the scheduler re-runs and
    /// resumes here in ~µs instead of waiting on the next timer tick — the
    /// cold-start fix for the inject pipeline.
    WaitingKick { kgen: u64, deadline: u64 },
    Done,
}

/// Per-core scheduler-loop context: saved on `switch` INTO a fiber,
/// restored when the fiber yields (`npk_sleep`) or finishes
/// (`fiber_on_exit`). Indexed by core id; only that core touches it.
static mut SCHED_CTX: [Context; MAX_CORES] = [Context::empty(); MAX_CORES];

/// The fiber currently executing on each core — a raw ptr into the `Box`
/// the scheduler checked out of the queue for the run's duration. Read by
/// `fiber_app_entry` / `npk_sleep` / `fiber_on_exit` to find "self".
static mut CURRENT_FIBER: [*mut Fiber; MAX_CORES] =
    [core::ptr::null_mut(); MAX_CORES];

/// Per-core run queue (Ready + Sleeping fibers). Only the owning core
/// touches it; the lock guards a possible Stage-5 ISR-driven wakeup and is
/// NEVER held across a `switch`.
static FIBER_QUEUES: [Mutex<VecDeque<Box<Fiber>>>; MAX_CORES] =
    [const { Mutex::new(VecDeque::new()) }; MAX_CORES];

/// Admit a freshly-spawned app as a Ready fiber on this core. Called from
/// the worker loop (`smp_ap_entry`) for `is_fiber` tasks.
pub fn admit(cid: usize, func: fn(u64), arg: u64) {
    admit_with_stack(cid, func, arg, DEFAULT_STACK_BYTES);
}

/// Like `admit` but with an explicit stack size. Use for long-running kernel
/// workers that run deep call chains (e.g. the 9p persist worker doing npkFS
/// writes/commits — AES + B-tree COW + journal — which the main kernel stack
/// handles fine but overflow the default 128 KiB fiber stack, silently smashing
/// memory since fibers have no guard page).
pub fn admit_with_stack(cid: usize, func: fn(u64), arg: u64, stack_bytes: usize) {
    if cid >= MAX_CORES {
        func(arg); // degenerate (no per-core queue) → run inline
        return;
    }
    let mut fiber = Box::new(Fiber::new(stack_bytes, fiber_app_entry, 0));
    fiber.app_func = Some(func);
    fiber.app_arg = arg;
    fiber.state = FiberState::Ready;
    FIBER_QUEUES[cid].lock().push_back(fiber);
}

/// Run this core's runnable fibers round-robin until none are runnable,
/// waking any whose sleep deadline has passed. Returns when every fiber is
/// sleeping (future deadline) or the queue is empty — the caller then idles
/// on the worker timer and re-enters next tick.
pub fn run_core_fibers(cid: usize) {
    if cid >= MAX_CORES {
        return;
    }
    loop {
        let now = crate::interrupts::rdtsc();
        // Check out the next runnable fiber (FIFO). Lock dropped before the
        // switch — the fiber's Box is owned by this stack frame meanwhile.
        let mut fiber = {
            let mut q = FIBER_QUEUES[cid].lock();
            let idx = q.iter().position(|f| match f.state {
                FiberState::Ready => true,
                FiberState::Sleeping(d) => now >= d,
                FiberState::WaitingIrq { vector, since, deadline } => {
                    crate::irq::fired_count(vector) != since || now >= deadline
                }
                FiberState::WaitingKick { kgen, deadline } => {
                    net_kick_gen(cid) != kgen || now >= deadline
                }
                FiberState::Done => false,
            });
            match idx {
                Some(i) => q.remove(i).unwrap(),
                None => return, // nothing runnable → idle
            }
        };

        let fptr: *mut Fiber = &mut *fiber;
        // SAFETY: fptr is stable across the switch (the Box is this frame's
        // local). Save the loop ctx into SCHED_CTX[cid], switch into the
        // fiber; control returns here when it yields (npk_sleep) or finishes
        // (fiber_on_exit), with `state` already updated.
        unsafe {
            CURRENT_FIBER[cid] = fptr;
            switch(&raw mut SCHED_CTX[cid], &raw const (*fptr).ctx);
            CURRENT_FIBER[cid] = core::ptr::null_mut();
        }

        if fiber.state == FiberState::Done {
            drop(fiber); // _start returned → free the stack
        } else {
            FIBER_QUEUES[cid].lock().push_back(fiber);
        }
    }
}

/// Park the running fiber for `ms` and yield its core to peer fibers.
/// Returns (resumes) once the scheduler re-runs it past the deadline.
/// Returns `false` if not running inside a fiber (caller falls back to an
/// in-place wait). This is the heart of Stage 2b — called by `npk_sleep`.
pub fn yield_sleep(ms: u64) -> bool {
    let cid = crate::smp::per_core::current_core_id();
    if cid >= MAX_CORES {
        return false;
    }
    // SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
    let f = unsafe { CURRENT_FIBER[cid] };
    if f.is_null() {
        return false;
    }
    let freq = crate::interrupts::tsc_freq();
    let deadline = crate::interrupts::rdtsc() + ms.saturating_mul(freq / 1000);
    // SAFETY: f is the running fiber (owned by run_core_fibers' frame). Set
    // its state, then switch back to the core scheduler context. Resumes
    // here when run_core_fibers switches into it again past the deadline.
    unsafe {
        (*f).state = FiberState::Sleeping(deadline);
        switch(&raw mut (*f).ctx, &raw const SCHED_CTX[cid]);
    }
    true
}

/// Yield the running fiber but stay immediately runnable (Ready) — used by
/// a compute-heavy fiber (e.g. a guest vCPU between run-slices) to let peer
/// fibers on the core take a turn, then resume on the next scheduler pass.
/// Returns `false` if not running inside a fiber.
pub fn yield_ready() -> bool {
    let cid = crate::smp::per_core::current_core_id();
    if cid >= MAX_CORES {
        return false;
    }
    // SAFETY: CURRENT_FIBER[cid] non-null iff we run inside a fiber here.
    let f = unsafe { CURRENT_FIBER[cid] };
    if f.is_null() {
        return false;
    }
    // SAFETY: f is the running fiber; mark Ready and switch back to the
    // core scheduler, which round-robins on to the next runnable fiber.
    unsafe {
        (*f).state = FiberState::Ready;
        switch(&raw mut (*f).ctx, &raw const SCHED_CTX[cid]);
    }
    true
}

/// Park the running fiber until device-IRQ `vector` fires (its fired-count
/// moves past `since`) or `timeout_ms` elapses. Returns true if the IRQ
/// fired, false on timeout (or if not running inside a fiber). The caller
/// snapshots `since` via `irq::arm(vector)` BEFORE submitting the device
/// command, so an IRQ racing the park is never lost. The MSI-X targets this
/// core, so the IRQ itself wakes it from HLT → the scheduler resumes us.
pub fn irq_wait(vector: u8, since: u64, timeout_ms: u64) -> bool {
    let cid = crate::smp::per_core::current_core_id();
    if cid >= MAX_CORES {
        return false;
    }
    // SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
    let f = unsafe { CURRENT_FIBER[cid] };
    if f.is_null() {
        return false;
    }
    let freq = crate::interrupts::tsc_freq();
    let deadline = crate::interrupts::rdtsc() + timeout_ms.saturating_mul(freq / 1000);
    // SAFETY: f is the running fiber (owned by run_core_fibers' frame). Park
    // it, switch back to the scheduler; resumes here when the count advances
    // or the deadline passes.
    unsafe {
        (*f).state = FiberState::WaitingIrq { vector, since, deadline };
        switch(&raw mut (*f).ctx, &raw const SCHED_CTX[cid]);
    }
    // Resumed: the IRQ fired iff the count advanced past the snapshot.
    crate::irq::fired_count(vector) != since
}

/// Per-core net-kick generation. An off-vCPU producer (the net RX worker)
/// bumps the target core's counter via [`net_kick_bump`] right before sending
/// its VCPU_KICK IPI, so a consumer fiber parked in `kick_wait` on that core is
/// resumed event-driven (the IPI wakes the core out of HLT → the scheduler
/// re-runs → the bumped generation marks the fiber runnable) instead of
/// waiting on the next ~1–10 ms timer tick. This is the cold-start fix.
static NET_KICK_GEN: [AtomicU64; MAX_CORES] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; MAX_CORES]
};

/// Bump core `cid`'s net-kick generation (called from `kick_host_core`, before
/// the IPI, so the wake can't be lost: a fiber that snapshots after this sees
/// the new value and doesn't park).
pub fn net_kick_bump(cid: usize) {
    if cid < MAX_CORES {
        NET_KICK_GEN[cid].fetch_add(1, Ordering::Release);
    }
}

fn net_kick_gen(cid: usize) -> u64 {
    if cid < MAX_CORES { NET_KICK_GEN[cid].load(Ordering::Acquire) } else { 0 }
}

/// Park the running fiber until this core's net-kick generation advances (an
/// off-vCPU producer injected + kicked) or `timeout_ms` elapses. The snapshot
/// is taken HERE, after the caller has drained its inbound queue, so a kick
/// racing the park is never lost (it advances the gen past the snapshot →
/// resumes at once). Returns true if kicked, false on timeout / not-in-fiber.
pub fn kick_wait(timeout_ms: u64) -> bool {
    let cid = crate::smp::per_core::current_core_id();
    if cid >= MAX_CORES {
        return false;
    }
    // SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
    let f = unsafe { CURRENT_FIBER[cid] };
    if f.is_null() {
        return false;
    }
    let kgen = net_kick_gen(cid);
    let freq = crate::interrupts::tsc_freq();
    let deadline = crate::interrupts::rdtsc() + timeout_ms.saturating_mul(freq / 1000);
    // SAFETY: f is the running fiber (owned by run_core_fibers' frame). Park it,
    // switch to the scheduler; resumes when the gen advances or the deadline passes.
    unsafe {
        (*f).state = FiberState::WaitingKick { kgen, deadline };
        switch(&raw mut (*f).ctx, &raw const SCHED_CTX[cid]);
    }
    net_kick_gen(cid) != kgen
}

/// Fresh-fiber entry: run the app's `(func, arg)`. On return the trampoline
/// falls into `fiber_on_exit`.
extern "C" fn fiber_app_entry(_unused: u64) {
    let cid = crate::smp::per_core::current_core_id();
    // SAFETY: the scheduler set CURRENT_FIBER[cid] right before switching in.
    let f = unsafe { CURRENT_FIBER[cid] };
    if f.is_null() {
        return;
    }
    let (func, arg) = unsafe { ((*f).app_func, (*f).app_arg) };
    if let Some(func) = func {
        func(arg);
    }
}

/// A fiber: a saved context, the heap-backed stack it runs on, its
/// scheduling state, and the app entry to run. Dropping it frees the stack
/// — only when finished (Done) or never started, never while parked.
pub struct Fiber {
    pub ctx: Context,
    state: FiberState,
    app_func: Option<fn(u64)>,
    app_arg: u64,
    // 16-byte-aligned backing store (Box<[u128]> guarantees align 16, which
    // the ABI needs at the trampoline's `call`). Kept solely to free on drop.
    _stack: Box<[u128]>,
}

impl Fiber {
    /// Build a fiber that will start at `entry(arg)` on a fresh stack.
    /// The first `switch` into `self.ctx` runs `entry`.
    pub fn new(stack_bytes: usize, entry: extern "C" fn(u64), arg: u64) -> Fiber {
        let n = stack_bytes.div_ceil(16).max(64);
        let mut stack = alloc::vec![0u128; n].into_boxed_slice();

        let base = stack.as_mut_ptr() as usize;
        let top = base + n * 16; // 16-aligned (Box<[u128]>)

        // Seven u64 slots below `top`, mirroring what `switch` pops then
        // `ret`s through:  r15 r14 r13 r12 rbx rbp [return addr]
        // At trampoline entry rsp == top (16-aligned) → ABI-correct `call`.
        let sp0 = top - 56;
        // SAFETY: sp0..top lies inside the freshly allocated stack; we
        // write exactly the 7 machine words the switch/ret sequence reads.
        unsafe {
            let p = sp0 as *mut u64;
            *p.add(0) = 0; // r15
            *p.add(1) = 0; // r14
            *p.add(2) = arg; // r13 → rdi in trampoline
            *p.add(3) = entry as usize as u64; // r12 → call target
            *p.add(4) = 0; // rbx
            *p.add(5) = 0; // rbp
            *p.add(6) = fiber_trampoline as *const () as u64; // ret → trampoline
        }

        Fiber {
            ctx: Context { rsp: sp0 as u64 },
            state: FiberState::Ready,
            app_func: None,
            app_arg: 0,
            _stack: stack,
        }
    }
}

/// Swap into `to`, saving the current context into `from`. Returns when
/// some other context switches back into `from`.
///
/// SAFETY: `to` must reference a context produced by `Fiber::new` or a
/// prior `switch` out, and its backing stack must still be alive.
pub unsafe fn switch(from: *mut Context, to: *const Context) {
    // SAFETY: forwarded to the asm primitive under the caller's contract.
    unsafe { fiber_context_switch(from, to) }
}

// ── Boot self-test (Stage 1 validation) ───────────────────────────────
//
// Runs once on Core 0 at boot. Switches into a fiber, which switches back,
// twice — proving bidirectional resume, stack setup, and argument passing.
// Prints `[fiber] self-test OK` on success. A broken switch triple-faults
// here (loud + early), exactly where we want it during bring-up.

static ST_STEP: AtomicU64 = AtomicU64::new(0);
static mut ST_MAIN: Context = Context::empty();
static mut ST_FIBER: *mut Context = core::ptr::null_mut();

extern "C" fn st_fiber_entry(arg: u64) {
    // Ran at all (+1) with the argument intact (+1 more = 2).
    ST_STEP.fetch_add(if arg == 0xF1B0 { 2 } else { 1 }, Ordering::SeqCst);
    // SAFETY: ST_FIBER/ST_MAIN set by `self_test` before the first switch.
    unsafe { switch(ST_FIBER, &raw const ST_MAIN) };
    // Resumed a second time → +20.
    ST_STEP.fetch_add(20, Ordering::SeqCst);
    // SAFETY: same contract; never returns (abandoned after this).
    unsafe { switch(ST_FIBER, &raw const ST_MAIN) };
}

/// Stage-1 boot validation. Safe to call once on Core 0 after serial is up.
pub fn self_test() {
    let mut fiber = Fiber::new(DEFAULT_STACK_BYTES, st_fiber_entry, 0xF1B0);
    let fiber_ctx: *mut Context = &mut fiber.ctx;

    // SAFETY: single-threaded boot path on Core 0; the statics are touched
    // only here and by the fiber we drive synchronously below.
    unsafe {
        ST_FIBER = fiber_ctx;
        // Enter the fiber; it runs, then switches back here.
        switch(&raw mut ST_MAIN, fiber_ctx as *const Context);
        let s1 = ST_STEP.load(Ordering::SeqCst);
        // Resume it; it runs the tail, then switches back.
        switch(&raw mut ST_MAIN, fiber_ctx as *const Context);
        let s2 = ST_STEP.load(Ordering::SeqCst);

        if s1 == 2 && s2 == 22 {
            crate::kprintln!("[fiber] self-test OK (2x round-trip, arg + resume verified)");
        } else {
            crate::kprintln!("[fiber] self-test FAIL: s1={} (want 2) s2={} (want 22)", s1, s2);
        }
    }
    // `fiber` drops here: it is parked at its 2nd switch and will never be
    // resumed, so freeing its stack is sound.
}
