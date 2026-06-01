//! Stackful fibers (green threads) — Stage 1: the context-switch primitive.
//!
//! See `SCHEDULER_FIBERS.md`. A fiber is "a stack + a saved context that
//! runs until it yields". wasmi cannot be paused mid-`_start`, so we give
//! each app its own stack and switch the whole CPU context at the yield
//! points (`npk_sleep` / `npk_event_wait`). The very same primitive will
//! later host guest-vCPU run-loops (multicore microvm) — it only swaps
//! `rsp` + the callee-saved registers, so it is agnostic to what runs on
//! the stack.
//!
//! Stage 1 ships ONLY the switch + a boot self-test. Nothing in the live
//! app path uses it yet (Stage 2 wires `wasm_worker_task` onto a fiber).

use core::sync::atomic::{AtomicU64, Ordering};

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

/// Called by the trampoline when a fiber's entry function returns (Stage 2:
/// the app's `_start` ran to completion). Switches back to the owning
/// core's saved scheduler context so `run_app_fiber` resumes and reclaims
/// the stack. Never returns to the trampoline.
#[unsafe(no_mangle)]
pub extern "C" fn fiber_on_exit() {
    let cid = crate::smp::per_core::current_core_id();
    // SAFETY: `SCHED_CTX[cid]` was saved by this core's `run_app_fiber`
    // switch-in; `SCRATCH[cid]` is a throwaway save slot (this fiber is
    // finished and will never be resumed). cid indexes this core only.
    unsafe {
        switch(&raw mut SCRATCH[cid], &raw const SCHED_CTX[cid]);
    }
    // Unreachable: control resumes in `run_app_fiber` after its `switch`.
}

/// Default per-fiber stack size. 128 KiB — comfortable headroom over the
/// 64 KiB AP stacks apps already run wasmi on today (and the old nesting
/// stacked two wasmi instances on those 64 KiB). The WASM linear memory is
/// separate (on the heap), so this only holds the interpreter + host-fn
/// call frames. No guard page yet (heap-backed); overflow = corruption.
pub const DEFAULT_STACK_BYTES: usize = 128 * 1024;

// ── Per-core fiber dispatch (Stage 2a) ─────────────────────────────────

const MAX_CORES: usize = 256;

/// Each worker core's scheduler-loop context, saved when it switches INTO
/// an app fiber and restored when the fiber returns (via `fiber_on_exit`)
/// or yields (Stage 2b). Indexed by core id; only that core touches it.
static mut SCHED_CTX: [Context; MAX_CORES] = [Context::empty(); MAX_CORES];
/// Throwaway save target for a finishing fiber's dead context.
static mut SCRATCH: [Context; MAX_CORES] = [Context::empty(); MAX_CORES];
/// The (func, arg) a fresh app fiber should run, handed over by
/// `run_app_fiber` right before switching in and consumed by
/// `fiber_app_entry`. Per-core → no cross-core race (one fiber starts at a
/// time per core in 2a).
static mut PENDING_FN: [Option<fn(u64)>; MAX_CORES] = [None; MAX_CORES];
static mut PENDING_ARG: [u64; MAX_CORES] = [0; MAX_CORES];

/// Fresh-fiber entry: pick up the pending (func, arg) for this core and run
/// it. When it returns, the trampoline falls into `fiber_on_exit`.
extern "C" fn fiber_app_entry(_unused: u64) {
    let cid = crate::smp::per_core::current_core_id();
    // SAFETY: set by `run_app_fiber` on this core just before the switch in.
    let f = unsafe { PENDING_FN[cid].take() };
    let arg = unsafe { PENDING_ARG[cid] };
    if let Some(f) = f {
        f(arg);
    }
}

/// Run `func(arg)` as a fiber on this worker core. Called from the worker
/// scheduler loop (`smp_ap_entry`) for fiber tasks. Allocates a stack,
/// switches into it, and returns once the fiber finishes (`func` returns)
/// — at which point the stack is freed. If `func` is a resident app whose
/// `_start` never returns, this call never returns either: the worker is
/// dedicated to that fiber until Stage 2b makes `npk_sleep` yield back.
pub fn run_app_fiber(cid: usize, func: fn(u64), arg: u64) {
    if cid >= MAX_CORES {
        func(arg);
        return;
    }
    let fiber = Fiber::new(DEFAULT_STACK_BYTES, fiber_app_entry, 0);
    let fiber_ctx: *const Context = &fiber.ctx;
    // SAFETY: cid is this core's own index; the slots are per-core. The
    // switch saves the loop context into SCHED_CTX[cid] and enters the
    // fiber; it returns here when the fiber switches back via
    // `fiber_on_exit`. `fiber` (its stack) stays alive across the switch.
    unsafe {
        PENDING_FN[cid] = Some(func);
        PENDING_ARG[cid] = arg;
        switch(&raw mut SCHED_CTX[cid], fiber_ctx);
    }
    drop(fiber); // fiber finished → free its stack
}

/// A fiber: a saved context plus the heap-backed stack it runs on.
/// Dropping it frees the stack — only do that when the fiber is finished
/// or has never started (never while it is parked mid-execution and might
/// be resumed).
pub struct Fiber {
    pub ctx: Context,
    // 16-byte-aligned backing store (Box<[u128]> guarantees align 16, which
    // the ABI needs at the trampoline's `call`). Kept solely to free on drop.
    _stack: alloc::boxed::Box<[u128]>,
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
