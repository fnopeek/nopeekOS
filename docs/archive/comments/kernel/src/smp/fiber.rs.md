# `kernel/src/smp/fiber.rs` @ 5e0102684

## L1-17 · `use alloc::boxed::Box;`

```
//! Stackful fibers (green threads) for WASM apps.
//!
//! See `docs/plan/SCHEDULER_FIBERS.md`. A fiber is "a stack + a saved context that
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
```

## L24-26 · `#[repr(C)]`

```
/// Saved execution context. Only `rsp` lives here — the callee-saved
/// registers (rbx, rbp, r12–r15) are pushed onto the fiber's own stack by
/// `switch` and popped back on resume, System V style.
```

## L39-48 · `core::arch::global_asm!(`

```
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
```

## L86-89 · `#[unsafe(no_mangle)]`

```
/// Called by the trampoline when a fiber's entry function returns — the
/// app's `_start` ran to completion. Mark the fiber Done and switch back to
/// the owning core's scheduler context, which then frees the stack. Never
/// returns to the trampoline.
```

## L93-94 · `unsafe {`

```
// SAFETY: CURRENT_FIBER[cid] is the finishing fiber (set by the
// scheduler before switching in); SCHED_CTX[cid] is the core's loop ctx.
```

## L102-104 · `}`

```
// Unreachable: control resumes in `run_core_fibers` after its `switch`.
// (If CURRENT_FIBER was somehow null, fall through to the trampoline's
// own hlt loop.)
```

## L107-111 · `pub const DEFAULT_STACK_BYTES: usize = 128 * 1024;`

```
/// Default per-fiber stack size. 128 KiB — comfortable headroom over the
/// 64 KiB AP stacks apps already run wasmi on today (and the old nesting
/// stacked two wasmi instances on those 64 KiB). The WASM linear memory is
/// separate (on the heap), so this only holds the interpreter + host-fn
/// call frames. No guard page yet (heap-backed); overflow = corruption.
```

## L114-121 · `const MAX_CORES: usize = 256;`

```
// ── Per-core fiber scheduler (Stage 2b) ────────────────────────────────
//
// Each app is a fiber pinned to the core that admitted it. `npk_sleep`
// parks the running fiber (Waiting + a TSC wake-deadline) and switches
// back to the core's scheduler context, FREEING the core to run its other
// ready fibers. So dock+bar+loft+spell multiplex over a couple of workers
// instead of pinning a core each or nesting. No fiber migrates between
// cores, so each core owns its queue with no cross-core hot path.
```

## L128-131 · `Waiting { mask: u32, deadline: u64 },`

```
/// Parked until one of `mask`'s signal bits is set on the fiber's waker
/// (`signal`), or TSC `deadline` passes (`NO_DEADLINE` = never). The ONE
/// wait state: a sleep is a wait with an empty mask, an IRQ wait is
/// `SIG_IRQ` (the ISR signals the waiter), a net kick is `SIG_KICK`.
```

## L138-145 · `pub const SIG_EVENT: u32 = 1 << 0;`

```
// ── Wakers: how anything reaches a parked fiber ────────────────────────
//
// A fiber lives in its core's queue; nobody else can touch it. What another
// core, an ISR or Core 0 CAN touch is its waker — a slot with signal bits
// and the core the fiber lives on. `signal` sets bits and wakes that core
// (IPI if it is halted); the core's scheduler sees the bits and resumes the
// fiber. Before this, every such wake was a poll: a deadline the fiber
// set itself, re-checked on a tick.
```

## L147 · `pub const SIG_EVENT: u32 = 1 << 0;`

```
/// A window event or a terminal key is waiting for the app.
```

## L149 · `pub const SIG_IRQ: u32 = 1 << 1;`

```
/// The device IRQ this fiber waits on fired.
```

## L151 · `pub const SIG_KICK: u32 = 1 << 2;`

```
/// This core's net-kick generation advanced.
```

## L153 · `pub const SIG_TX: u32 = 1 << 3;`

```
/// The IP stack queued a frame for the WASM NIC driver.
```

## L155 · `pub const SIG_WIFI: u32 = 1 << 4;`

```
/// A wifi command (wifid → driver) or event (driver → wifid) was queued.
```

## L157 · `pub const SIG_STATE: u32 = 1 << 5;`

```
/// A watched topic changed (`crate::notify`).
```

## L186 · `NO_WAKER // table full: the fiber can still sleep, just not be signalled`

```
// table full: the fiber can still sleep, just not be signalled
```

## L201-202 · `pub fn signal(w: Waker, bits: u32) {`

```
/// Set `bits` on waker `w` and wake the core its fiber lives on. Callable
/// from any core and from interrupt context (lock-free).
```

## L212 · `pub fn current_waker() -> Option<Waker> {`

```
/// The running fiber's waker, or None outside a fiber.
```

## L218 · `let f = unsafe { CURRENT_FIBER[cid] };`

```
// SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
```

## L223 · `let w = unsafe { (*f).waker };`

```
// SAFETY: f is the running fiber.
```

## L228-232 · `pub fn wait(mask: u32, deadline: u64) -> Option<u32> {`

```
/// Park the running fiber until a bit of `mask` is signalled or TSC
/// `deadline` passes. Returns the signalled bits it consumed (0 = deadline),
/// or None when not running inside a fiber — the caller then waits another
/// way. Bits already set return at once, so a signal that raced ahead of
/// the park is not lost.
```

## L238 · `let f = unsafe { CURRENT_FIBER[cid] };`

```
// SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
```

## L243 · `let w = unsafe { (*f).waker };`

```
// SAFETY: f is the running fiber (owned by run_core_fibers' frame).
```

## L253-254 · `unsafe {`

```
// SAFETY: as above — park, switch to the scheduler; it resumes us when a
// bit of `mask` is set or the deadline passes.
```

## L262-264 · `static mut SCHED_CTX: [Context; MAX_CORES] = [Context::empty(); MAX_CORES];`

```
/// Per-core scheduler-loop context: saved on `switch` INTO a fiber,
/// restored when the fiber yields (`npk_sleep`) or finishes
/// (`fiber_on_exit`). Indexed by core id; only that core touches it.
```

## L267-269 · `static mut CURRENT_FIBER: [*mut Fiber; MAX_CORES] =`

```
/// The fiber currently executing on each core — a raw ptr into the `Box`
/// the scheduler checked out of the queue for the run's duration. Read by
/// `fiber_app_entry` / `npk_sleep` / `fiber_on_exit` to find "self".
```

## L273-275 · `static FIBER_QUEUES: [Mutex<VecDeque<Box<Fiber>>>; MAX_CORES] =`

```
/// Per-core run queue (Ready + Waiting fibers). Only the owning core
/// touches it; the lock guards a possible Stage-5 ISR-driven wakeup and is
/// NEVER held across a `switch`.
```

## L279-280 · `pub fn admit(cid: usize, func: fn(u64), arg: u64) {`

```
/// Admit a freshly-spawned app as a Ready fiber on this core. Called from
/// the worker loop (`smp_ap_entry`) for `is_fiber` tasks.
```

## L285-289 · `pub fn admit_with_stack(cid: usize, func: fn(u64), arg: u64, stack_bytes: usize) {`

```
/// Like `admit` but with an explicit stack size. Use for long-running kernel
/// workers that run deep call chains (e.g. the 9p persist worker doing npkFS
/// writes/commits — AES + B-tree COW + journal — which the main kernel stack
/// handles fine but overflow the default 128 KiB fiber stack, silently smashing
/// memory since fibers have no guard page).
```

## L292 · `func(arg); // degenerate (no per-core queue) → run inline`

```
// degenerate (no per-core queue) → run inline
```

## L302-305 · `core::sync::atomic::fence(Ordering::SeqCst);`

```
// A fiber placed on ANOTHER core (the microVM's AP vCPUs, the fetch /
// GPU / 9p / net workers) must wake that core: an idle worker has no
// tick any more and would not look at its queue. It sees the new Ready
// fiber in its idle re-check (`earliest_deadline`) or gets the IPI.
```

## L310-311 · `static FIBER_COUNT: [AtomicU64; MAX_CORES] = [const { AtomicU64::new(0) }; MAX_CORES];`

```
/// Resident fibers per core, readable from ANY core. The queue length is not:
/// while a core runs a fiber, that fiber is checked out of the queue.
```

## L314 · `pub fn fiber_count(cid: usize) -> u64 {`

```
/// How many fibers live on `cid` (running or parked).
```

## L320-339 · `pub fn pump_peers() {`

```
/// Run this core's runnable fibers round-robin until none are runnable,
/// waking any whose sleep deadline has passed. Returns when every fiber is
/// sleeping (future deadline) or the queue is empty — the caller then idles
/// on the worker timer and re-enters next tick.
/// Let this core's fibers run from inside a native task.
///
/// Fibers are cooperative and only ever run from `run_core_fibers`, which is
/// the core's scheduler loop. A native task — every intent is one — stands in
/// front of that loop for its whole duration, so a WASM driver whose fiber
/// lives on the same core stops polling its device until the command returns.
///
/// Measured, and it explains a day of ghosts: `ping` reported 100 % loss and
/// then printed all four replies AFTER the command finished; a DNS lookup saw
/// zero datagrams in 5.5 s and the same name resolved instantly on the next
/// try. The frames were in the card's ring the whole time (rx drain-peak 55 of
/// 64 buffers on an idle link) with nobody scheduled to fetch them.
///
/// Safe to call from a native task: the scheduler context for this core is
/// free precisely because no fiber is running on it. A no-op inside a fiber
/// (that would recurse) and on Core 0, which has its own loop.
```

## L341-346 · `const EVERY: u32 = 256;`

```
// Throttled, because the check itself is not free: current_core_id reads the
// LAPIC over MMIO, and `poll_rx_only` spins about a million times a second.
// The comment above that function already warns about exactly this cost — a
// per-iteration core gate was once 30 % of its runtime — and the first
// version of this pump walked straight back into it. Yielding every 256th
// pass is just as good: it is a courtesy, not a deadline.
```

## L355 · `if unsafe { !CURRENT_FIBER[cid].is_null() } {`

```
// SAFETY: per-core slot, read from the core it belongs to.
```

## L357 · `return; // already inside a fiber — the loop below is our own caller`

```
// already inside a fiber — the loop below is our own caller
```

## L370-371 · `let mut fiber = {`

```
// Check out the next runnable fiber (FIFO). Lock dropped before the
// switch — the fiber's Box is owned by this stack frame meanwhile.
```

## L383 · `None => return, // nothing runnable → idle`

```
// nothing runnable → idle
```

## L388-391 · `unsafe {`

```
// SAFETY: fptr is stable across the switch (the Box is this frame's
// local). Save the loop ctx into SCHED_CTX[cid], switch into the
// fiber; control returns here when it yields (npk_sleep) or finishes
// (fiber_on_exit), with `state` already updated.
```

## L401 · `drop(fiber); // _start returned → free the stack`

```
// _start returned → free the stack
```

## L408-415 · `pub fn earliest_deadline(cid: usize) -> Option<u64> {`

```
/// The earliest TSC deadline any fiber on this core is waiting for — 0 for
/// one that is Ready — or None if nothing is waiting on time.
///
/// The idle path needs this. A fiber that asks for a 1 ms sleep is otherwise
/// resumed only by the next 100 Hz worker tick, because `run_core_fibers`
/// returns and the core HLTs — so every sub-10 ms sleep silently becomes 10 ms.
/// For a polling driver that turns its poll period, and with it its throughput
/// ceiling, into a property of the timer rate rather than of the device.
```

## L423 · `FiberState::Ready => Some(0),`

```
// Due now: a fiber admitted after `run_core_fibers` returned.
```

## L425-426 · `FiberState::Waiting { mask, .. } if pending(f.waker, mask) => Some(0),`

```
// Signalled but not yet resumed: due now. A signal from another
// core that raced this core into its idle path lands here.
```

## L435-437 · `pub fn yield_sleep(ms: u64) -> bool {`

```
/// Park the running fiber for `ms` and yield its core to peer fibers.
/// Returns `false` if not running inside a fiber (caller falls back to an
/// in-place wait). Called by `npk_sleep`: a wait with an empty mask.
```

## L444-447 · `pub fn yield_ready() -> bool {`

```
/// Yield the running fiber but stay immediately runnable (Ready) — used by
/// a compute-heavy fiber (e.g. a guest vCPU between run-slices) to let peer
/// fibers on the core take a turn, then resume on the next scheduler pass.
/// Returns `false` if not running inside a fiber.
```

## L453 · `let f = unsafe { CURRENT_FIBER[cid] };`

```
// SAFETY: CURRENT_FIBER[cid] non-null iff we run inside a fiber here.
```

## L458-459 · `unsafe {`

```
// SAFETY: f is the running fiber; mark Ready and switch back to the
// core scheduler, which round-robins on to the next runnable fiber.
```

## L467-472 · `pub fn irq_wait(vector: u8, since: u64, timeout_ms: u64) -> bool {`

```
/// Park the running fiber until device-IRQ `vector` fires (its fired-count
/// moves past `since`) or `timeout_ms` elapses. Returns true if the IRQ
/// fired, false on timeout (or if not running inside a fiber). The caller
/// snapshots `since` via `irq::arm(vector)` BEFORE submitting the device
/// command. The MSI-X targets this core; the ISR signals the registered
/// waiter (`irq::set_waiter`), which ends the park.
```

## L477-478 · `crate::irq::set_waiter(vector, w);`

```
// Register BEFORE looking at the count: an IRQ after the look then
// signals us, and `wait` returns at once.
```

## L493-498 · `static NET_KICK_GEN: [AtomicU64; MAX_CORES] = {`

```
/// Per-core net-kick generation. An off-vCPU producer (the net RX worker)
/// bumps the target core's counter via [`net_kick_bump`] right before sending
/// its VCPU_KICK IPI, so a consumer fiber parked in `kick_wait` on that core is
/// resumed event-driven (the IPI wakes the core out of HLT → the scheduler
/// re-runs → the bumped generation marks the fiber runnable) instead of
/// waiting on the next ~1–10 ms timer tick. This is the cold-start fix.
```

## L504-506 · `pub fn net_kick_bump(cid: usize) {`

```
/// Bump core `cid`'s net-kick generation (called from `kick_host_core`, before
/// the IPI, so the wake can't be lost: a fiber that snapshots after this sees
/// the new value and doesn't park).
```

## L510-511 · `let _ = KICK_SENT_TSC[cid].compare_exchange(`

```
// probe: stamp the FIRST kick after a park began (CAS 0→now); cleared at
// park start in kick_wait_until, read on resume → kick→resume latency.
```

## L521-524 · `static KICK_WAITER: [AtomicU32; MAX_CORES] = [const { AtomicU32::new(NO_WAKER) }; MAX_CORES];`

```
/// The fiber that waits in `kick_wait_until` on each core. ONE per core —
/// in practice there is one (a vCPU fiber, or the net worker on its reserved
/// core). A second one would only be resumed by its deadline, and every
/// kick wait has one of a few milliseconds.
```

## L527 · `pub fn net_kick_gen(cid: usize) -> u64 {`

```
/// Core `cid`'s kick generation — a cheap "was I kicked since" test.
```

## L532-533 · `static KICK_WOKE: AtomicU64 = AtomicU64::new(0);`

```
/// kick_wait wakeup attribution: woke = a kick advanced the gen (event-driven,
/// µs); timeout = fell to the 2ms fallback (no kick arrived → the cold floor).
```

## L536 · `pub fn kick_wait_snapshot() -> (u64, u64) {`

```
/// (woke, timeout) counts — double-sample for a rate.
```

## L541-546 · `static KICK_SENT_TSC: [AtomicU64; MAX_CORES] = {`

```
/// kick→resume LATENCY probe (the irqfd-gap measurement): per-core TSC of the
/// first net-kick that lands AFTER a fiber begins parking (cleared at park start,
/// CAS-set in `net_kick_bump`). On resume the parked fiber reads it → delta = how
/// long the kick took to actually reschedule this core. µs ⇒ the IPI woke it
/// promptly (the ~3 ms RTT is elsewhere); ms ⇒ the kicked-but-HLTed core waited
/// for the host scheduler / nested-IPI delivery — exactly what KVM's irqfd avoids.
```

## L554 · `pub fn kick_latency_snapshot() -> (u64, u64, u64) {`

```
/// (sum_tsc, n, max_tsc) — max is swap-reset so `cores` reads the window peak.
```

## L561-565 · `pub fn kick_wait(timeout_ms: u64) -> bool {`

```
/// Park the running fiber until this core's net-kick generation advances (an
/// off-vCPU producer injected + kicked) or `timeout_ms` elapses. The snapshot
/// is taken HERE, after the caller has drained its inbound queue, so a kick
/// racing the park is never lost (it advances the gen past the snapshot →
/// resumes at once). Returns true if kicked, false on timeout / not-in-fiber.
```

## L572-577 · `pub fn kick_wait_until(deadline: u64) -> bool {`

```
/// Park the running fiber until this core's net-kick generation advances (an
/// off-vCPU producer / IPI kicked us) or absolute host TSC `deadline` passes.
/// The deadline variant is the vCPU-park form: the caller passes the guest's
/// next LAPIC-timer deadline (`LocalApic::next_timer_deadline_tsc`), so the park
/// ends exactly at the guest's own 1 kHz tick — the KVM hrtimer model — instead
/// of a magic fixed timeout. Returns true if a kick woke us, false on deadline.
```

## L583 · `let f = unsafe { CURRENT_FIBER[cid] };`

```
// SAFETY: CURRENT_FIBER[cid] is non-null iff we run inside a fiber here.
```

## L588 · `let me = unsafe { (*f).waker };`

```
// SAFETY: f is the running fiber.
```

## L591 · `KICK_SENT_TSC[cid].store(0, Ordering::Relaxed); // probe: measure kicks from here`

```
// probe: measure kicks from here
```

## L592-593 · `KICK_WAITER[cid].store(me, Ordering::Release);`

```
// Register before the loop's look at the generation: a bump after the
// look signals us, and `wait` returns at once.
```

## L618-619 · `extern "C" fn fiber_app_entry(_unused: u64) {`

```
/// Fresh-fiber entry: run the app's `(func, arg)`. On return the trampoline
/// falls into `fiber_on_exit`.
```

## L622 · `let f = unsafe { CURRENT_FIBER[cid] };`

```
// SAFETY: the scheduler set CURRENT_FIBER[cid] right before switching in.
```

## L633-635 · `pub struct Fiber {`

```
/// A fiber: a saved context, the heap-backed stack it runs on, its
/// scheduling state, and the app entry to run. Dropping it frees the stack
/// — only when finished (Done) or never started, never while parked.
```

## L642-643 · `_stack: Box<[u128]>,`

```
// 16-byte-aligned backing store (Box<[u128]> guarantees align 16, which
// the ABI needs at the trampoline's `call`). Kept solely to free on drop.
```

## L648-649 · `pub fn new(stack_bytes: usize, entry: extern "C" fn(u64), arg: u64) -> Fiber {`

```
/// Build a fiber that will start at `entry(arg)` on a fresh stack.
/// The first `switch` into `self.ctx` runs `entry`.
```

## L655 · `let top = base + n * 16; // 16-aligned (Box<[u128]>)`

```
// 16-aligned (Box<[u128]>)
```

## L657-659 · `let sp0 = top - 56;`

```
// Seven u64 slots below `top`, mirroring what `switch` pops then
// `ret`s through:  r15 r14 r13 r12 rbx rbp [return addr]
// At trampoline entry rsp == top (16-aligned) → ABI-correct `call`.
```

## L661-662 · `unsafe {`

```
// SAFETY: sp0..top lies inside the freshly allocated stack; we
// write exactly the 7 machine words the switch/ret sequence reads.
```

## L665 · `*p.add(0) = 0; // r15`

```
// r15
```

## L666 · `*p.add(1) = 0; // r14`

```
// r14
```

## L667 · `*p.add(2) = arg; // r13 → rdi in trampoline`

```
// r13 → rdi in trampoline
```

## L668 · `*p.add(3) = entry as usize as u64; // r12 → call target`

```
// r12 → call target
```

## L669 · `*p.add(4) = 0; // rbx`

```
// rbx
```

## L670 · `*p.add(5) = 0; // rbp`

```
// rbp
```

## L671 · `*p.add(6) = fiber_trampoline as *const () as u64; // ret → trampoline`

```
// ret → trampoline
```

## L685-689 · `pub unsafe fn switch(from: *mut Context, to: *const Context) {`

```
/// Swap into `to`, saving the current context into `from`. Returns when
/// some other context switches back into `from`.
///
/// SAFETY: `to` must reference a context produced by `Fiber::new` or a
/// prior `switch` out, and its backing stack must still be alive.
```

## L691 · `unsafe { fiber_context_switch(from, to) }`

```
// SAFETY: forwarded to the asm primitive under the caller's contract.
```

## L695-700 · `static ST_STEP: AtomicU64 = AtomicU64::new(0);`

```
// ── Boot self-test (Stage 1 validation) ───────────────────────────────
//
// Runs once on Core 0 at boot. Switches into a fiber, which switches back,
// twice — proving bidirectional resume, stack setup, and argument passing.
// Prints `[fiber] self-test OK` on success. A broken switch triple-faults
// here (loud + early), exactly where we want it during bring-up.
```

## L707 · `ST_STEP.fetch_add(if arg == 0xF1B0 { 2 } else { 1 }, Ordering::SeqCst);`

```
// Ran at all (+1) with the argument intact (+1 more = 2).
```

## L709 · `unsafe { switch(ST_FIBER, &raw const ST_MAIN) };`

```
// SAFETY: ST_FIBER/ST_MAIN set by `self_test` before the first switch.
```

## L711 · `ST_STEP.fetch_add(20, Ordering::SeqCst);`

```
// Resumed a second time → +20.
```

## L713 · `unsafe { switch(ST_FIBER, &raw const ST_MAIN) };`

```
// SAFETY: same contract; never returns (abandoned after this).
```

## L717 · `pub fn self_test() {`

```
/// Stage-1 boot validation. Safe to call once on Core 0 after serial is up.
```

## L722-723 · `unsafe {`

```
// SAFETY: single-threaded boot path on Core 0; the statics are touched
// only here and by the fiber we drive synchronously below.
```

## L726 · `switch(&raw mut ST_MAIN, fiber_ctx as *const Context);`

```
// Enter the fiber; it runs, then switches back here.
```

## L729 · `switch(&raw mut ST_MAIN, fiber_ctx as *const Context);`

```
// Resume it; it runs the tail, then switches back.
```

## L739-740 · `}`

```
// `fiber` drops here: it is parked at its 2nd switch and will never be
// resumed, so freeing its stack is sound.
```

